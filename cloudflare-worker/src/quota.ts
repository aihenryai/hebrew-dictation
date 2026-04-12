/**
 * Per-device monthly quota tracking, backed by Cloudflare KV.
 *
 * Free tier: FREE_TIER_MINUTES minutes/month (default 30).
 * Email-claimed bonus: doubles the free quota (added EMAIL_BONUS_MINUTES on top).
 */

import type { DeviceRecord, Env } from "./types";

const FREE_TIER_DEFAULT = 30;
const EMAIL_BONUS_DEFAULT = 30;

/**
 * Returns ISO timestamp of the start of next UTC month.
 * Used to schedule quota resets.
 */
export function startOfNextMonth(): string {
  const now = new Date();
  return new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth() + 1, 1, 0, 0, 0)
  ).toISOString();
}

/**
 * Loads a device record. Creates a fresh one if missing.
 * Resets the monthly counter automatically if the reset date has passed.
 */
export async function getOrCreateDevice(
  env: Env,
  deviceHash: string
): Promise<DeviceRecord> {
  const existing = (await env.DEVICES.get(deviceHash, "json")) as DeviceRecord | null;
  const nowIso = new Date().toISOString();

  if (!existing) {
    const fresh: DeviceRecord = {
      device_hash: deviceHash,
      minutes_used_this_month: 0,
      month_reset_at: startOfNextMonth(),
      email_claimed: false,
      created_at: nowIso,
    };
    await env.DEVICES.put(deviceHash, JSON.stringify(fresh));
    return fresh;
  }

  // Auto-reset on month rollover
  if (new Date(existing.month_reset_at).getTime() <= Date.now()) {
    existing.minutes_used_this_month = 0;
    existing.month_reset_at = startOfNextMonth();
    await env.DEVICES.put(deviceHash, JSON.stringify(existing));
  }

  return existing;
}

/**
 * Computes the monthly minute limit for a device — base + email bonus if claimed.
 */
export function getQuotaLimit(env: Env, device: DeviceRecord): number {
  const base = parseInt(env.FREE_TIER_MINUTES, 10) || FREE_TIER_DEFAULT;
  const bonus = parseInt(env.EMAIL_BONUS_MINUTES, 10) || EMAIL_BONUS_DEFAULT;
  return device.email_claimed ? base + bonus : base;
}

/**
 * Increments the device's monthly usage by the given minutes and persists.
 * Returns the updated record.
 */
export async function consumeMinutes(
  env: Env,
  device: DeviceRecord,
  minutes: number
): Promise<DeviceRecord> {
  device.minutes_used_this_month += minutes;
  await env.DEVICES.put(device.device_hash, JSON.stringify(device));
  return device;
}

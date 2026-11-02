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
 * Read-only view of a device for GET /quota: never writes KV, so an anonymous caller cycling
 * device hashes cannot burn the namespace's daily write allowance (security audit 2026-11-02).
 */
export async function peekDevice(
  env: Env,
  deviceHash: string
): Promise<DeviceRecord> {
  const existing = (await env.DEVICES.get(deviceHash, "json")) as DeviceRecord | null;
  if (existing && new Date(existing.month_reset_at).getTime() > Date.now()) return existing;
  return {
    device_hash: deviceHash,
    minutes_used_this_month: 0,
    month_reset_at: startOfNextMonth(),
    email_claimed: existing?.email_claimed ?? false,
    created_at: existing?.created_at ?? new Date().toISOString(),
  };
}

/**
 * The device hash is chosen by the client, so a per-device quota alone does not bound spend:
 * anyone can send a fresh hash with every request and transcribe for free on Henry's Groq /
 * Deepgram key (security audit 2026-11-02). Two server-side ceilings close that:
 *   - every device together: GLOBAL_DAILY_MINUTES a UTC day (default 120);
 *   - new devices from one IP: NEW_DEVICES_PER_IP_PER_DAY (default 3).
 */
const GLOBAL_DAILY_DEFAULT = 120;
const NEW_DEVICES_PER_IP_DEFAULT = 3;
const DAY_TTL_SECONDS = 2 * 24 * 60 * 60;

const utcDay = () => new Date().toISOString().slice(0, 10);

export async function globalMinutesLeft(env: Env): Promise<number> {
  const cap = parseFloat(env.GLOBAL_DAILY_MINUTES ?? "") || GLOBAL_DAILY_DEFAULT;
  const used = parseFloat((await env.DEVICES.get(`global:${utcDay()}`)) ?? "0") || 0;
  return cap - used;
}

export async function addGlobalMinutes(env: Env, minutes: number): Promise<void> {
  const key = `global:${utcDay()}`;
  const used = parseFloat((await env.DEVICES.get(key)) ?? "0") || 0;
  await env.DEVICES.put(key, String(used + minutes), { expirationTtl: DAY_TTL_SECONDS });
}

/** True when this IP may register one more new device today (and counts it). */
export async function allowNewDevice(env: Env, ip: string): Promise<boolean> {
  const cap = parseInt(env.NEW_DEVICES_PER_IP_PER_DAY ?? "", 10) || NEW_DEVICES_PER_IP_DEFAULT;
  const key = `newdev:${utcDay()}:${ip}`;
  const count = parseInt((await env.DEVICES.get(key)) ?? "0", 10) || 0;
  if (count >= cap) return false;
  await env.DEVICES.put(key, String(count + 1), { expirationTtl: DAY_TTL_SECONDS });
  return true;
}

export async function deviceExists(env: Env, deviceHash: string): Promise<boolean> {
  return (await env.DEVICES.get(deviceHash)) !== null;
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

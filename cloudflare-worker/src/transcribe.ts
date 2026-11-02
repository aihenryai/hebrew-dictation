/**
 * /transcribe and /quota request handlers.
 */

import type {
  Env,
  ErrorResponse,
  QuotaResponse,
  TranscribeResponse,
} from "./types";
import {
  addGlobalMinutes,
  allowNewDevice,
  consumeMinutes,
  deviceExists,
  getOrCreateDevice,
  getQuotaLimit,
  globalMinutesLeft,
  peekDevice,
} from "./quota";
import { transcribeGroq } from "./groq-client";
import { transcribeDeepgram } from "./deepgram-client";

// The hash doubles as a KV key: a fixed alphabet and length keep "global:..." / "newdev:..." out of reach.
const DEVICE_HASH_RE = /^[A-Za-z0-9_-]{16,128}$/;
const MAX_AUDIO_BYTES = 25 * 1024 * 1024; // 25MB hard cap (Groq limit)

/**
 * POST /transcribe
 * Headers:
 *   X-Device-Hash: <hash from Tauri client>
 *   X-Audio-Duration-Seconds: <float, computed by client from WAV header>
 * Body: raw audio bytes (WAV preferred)
 */
export async function handleTranscribe(
  request: Request,
  env: Env
): Promise<Response> {
  const deviceHash = request.headers.get("X-Device-Hash");
  if (!deviceHash || !DEVICE_HASH_RE.test(deviceHash)) {
    return jsonError(400, "Missing or invalid X-Device-Hash header");
  }

  const declared = Number(request.headers.get("Content-Length") ?? "0");
  if (declared > MAX_AUDIO_BYTES) {
    return jsonError(413, `Audio exceeds ${MAX_AUDIO_BYTES} byte limit`);
  }

  const audio = await request.arrayBuffer();
  if (audio.byteLength === 0) {
    return jsonError(400, "Empty audio payload");
  }
  if (audio.byteLength > MAX_AUDIO_BYTES) {
    return jsonError(413, `Audio exceeds ${MAX_AUDIO_BYTES} byte limit`);
  }

  // Always compute duration server-side from audio byte size (never trust client headers).
  const minutesUsed = computeMinutesFromAudioSize(audio.byteLength);

  if ((await globalMinutesLeft(env)) < minutesUsed) {
    return jsonError(503, "Service busy, try again later");
  }
  if (!(await deviceExists(env, deviceHash))) {
    const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
    if (!(await allowNewDevice(env, ip))) {
      return jsonError(429, "Too many new devices from this network today");
    }
  }

  const device = await getOrCreateDevice(env, deviceHash);
  const limit = getQuotaLimit(env, device);

  if (device.minutes_used_this_month + minutesUsed > limit) {
    return jsonError(402, "Monthly quota exceeded", {
      tier: "free" as const,
      minutes_used: device.minutes_used_this_month,
      minutes_remaining: Math.max(0, limit - device.minutes_used_this_month),
      month_reset_at: device.month_reset_at,
      email_claimed: device.email_claimed,
    });
  }

  let text: string;
  try {
    text =
      env.BACKEND === "deepgram"
        ? await transcribeDeepgram(audio, env.DEEPGRAM_API_KEY)
        : await transcribeGroq(audio, env.GROQ_API_KEY, env.GROQ_MODEL);
  } catch (e) {
    console.error("Transcription backend error:", (e as Error).message);
    return jsonError(502, "Transcription service temporarily unavailable");
  }

  const updated = await consumeMinutes(env, device, minutesUsed);
  await addGlobalMinutes(env, minutesUsed);
  const remaining = Math.max(0, limit - updated.minutes_used_this_month);

  const body: TranscribeResponse = {
    text,
    minutes_used: updated.minutes_used_this_month,
    minutes_remaining: remaining,
  };
  return json(200, body);
}

/**
 * GET /quota
 * Headers:
 *   X-Device-Hash: <hash from Tauri client>
 */
export async function handleQuota(
  request: Request,
  env: Env
): Promise<Response> {
  const deviceHash = request.headers.get("X-Device-Hash");
  if (!deviceHash || !DEVICE_HASH_RE.test(deviceHash)) {
    return jsonError(400, "Missing or invalid X-Device-Hash header");
  }

  const device = await peekDevice(env, deviceHash);
  const limit = getQuotaLimit(env, device);

  const body: QuotaResponse = {
    tier: "free",
    minutes_used: device.minutes_used_this_month,
    minutes_remaining: Math.max(0, limit - device.minutes_used_this_month),
    month_reset_at: device.month_reset_at,
    email_claimed: device.email_claimed,
  };
  return json(200, body);
}

/**
 * Computes minutes consumed for quota purposes from audio byte size.
 * Estimate assumes 16kHz mono PCM s16 (~1.875 MB/min).
 * Rounds up to 0.1 min minimum to prevent abuse via tiny clips.
 */
function computeMinutesFromAudioSize(audioBytes: number): number {
  const estimated = audioBytes / (1.875 * 1024 * 1024);
  return Math.max(0.1, estimated);
}

function json(status: number, body: object): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function jsonError(status: number, message: string, extra?: object): Response {
  const body: ErrorResponse = { error: message, ...(extra || {}) };
  return json(status, body);
}

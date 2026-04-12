/**
 * /transcribe and /quota request handlers.
 */

import type {
  Env,
  ErrorResponse,
  QuotaResponse,
  TranscribeResponse,
} from "./types";
import { consumeMinutes, getOrCreateDevice, getQuotaLimit } from "./quota";
import { transcribeGroq } from "./groq-client";
import { transcribeDeepgram } from "./deepgram-client";

const MIN_DEVICE_HASH_LENGTH = 16;
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
  if (!deviceHash || deviceHash.length < MIN_DEVICE_HASH_LENGTH) {
    return jsonError(400, "Missing or invalid X-Device-Hash header");
  }

  const audio = await request.arrayBuffer();
  if (audio.byteLength === 0) {
    return jsonError(400, "Empty audio payload");
  }
  if (audio.byteLength > MAX_AUDIO_BYTES) {
    return jsonError(413, `Audio exceeds ${MAX_AUDIO_BYTES} byte limit`);
  }

  // Prefer client-provided duration; fall back to byte-size estimate.
  const minutesUsed = computeMinutesFromRequest(request, audio.byteLength);

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
    return jsonError(502, `Transcription backend failed: ${(e as Error).message}`);
  }

  const updated = await consumeMinutes(env, device, minutesUsed);
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
  if (!deviceHash || deviceHash.length < MIN_DEVICE_HASH_LENGTH) {
    return jsonError(400, "Missing or invalid X-Device-Hash header");
  }

  const device = await getOrCreateDevice(env, deviceHash);
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
 * Computes minutes consumed for quota purposes.
 * Strategy:
 *   1. If client sent X-Audio-Duration-Seconds header, trust it (most accurate).
 *   2. Otherwise, estimate from byte size assuming 16kHz mono PCM s16 (~1.875MB/min).
 * Always rounds up to 0.1 min minimum to prevent abuse via tiny clips.
 */
function computeMinutesFromRequest(
  request: Request,
  audioBytes: number
): number {
  const durationHeader = request.headers.get("X-Audio-Duration-Seconds");
  if (durationHeader) {
    const seconds = parseFloat(durationHeader);
    if (!isNaN(seconds) && seconds > 0) {
      return Math.max(0.1, seconds / 60);
    }
  }
  // Fallback: 1.875MB ≈ 1 minute of 16kHz mono PCM s16
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

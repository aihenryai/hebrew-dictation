/**
 * Shared type definitions for the Hebrew Dictation API Worker.
 */

export interface Env {
  // KV namespaces
  DEVICES: KVNamespace;
  EMAILS: KVNamespace;
  LICENSES: KVNamespace;

  // Secrets (set via `wrangler secret put`)
  GROQ_API_KEY: string;
  DEEPGRAM_API_KEY: string;

  // Vars (from wrangler.toml)
  FREE_TIER_MINUTES: string;
  EMAIL_BONUS_MINUTES: string;
  GROQ_MODEL: string;
  BACKEND: "groq" | "deepgram";
}

/**
 * A device record stored in the DEVICES KV.
 * Identifies a free-tier user via a hash of their machine GUID + username.
 */
export interface DeviceRecord {
  device_hash: string;
  minutes_used_this_month: number;
  month_reset_at: string; // ISO timestamp
  email_claimed: boolean;
  linked_email_hash?: string;
  created_at: string; // ISO timestamp
}

/**
 * Response shape for GET /quota — sent to the Tauri client to render the quota UI.
 */
export interface QuotaResponse {
  tier: "free" | "pro";
  minutes_used: number;
  minutes_remaining: number;
  month_reset_at: string;
  email_claimed: boolean;
}

/**
 * Response shape for POST /transcribe.
 */
export interface TranscribeResponse {
  text: string;
  minutes_used: number;
  minutes_remaining: number;
}

export interface ErrorResponse {
  error: string;
  [extra: string]: unknown;
}

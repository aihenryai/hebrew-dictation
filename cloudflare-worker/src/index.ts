/**
 * Hebrew Dictation API — Cloudflare Worker entrypoint.
 *
 * Routes:
 *   POST /transcribe  → audio upload, returns transcript + quota state
 *   GET  /quota       → returns current quota state for a device
 *   GET  /health      → returns "ok" (smoke test)
 *
 * Identity: device-hash for free tier (Phase 1).
 * Email claim flow (/claim-email, /verify-email) added in Phase 3.
 * Payments (/webhooks/lemonsqueezy, /license/validate) added in Phase 4.
 */

import type { Env } from "./types";
import { handleQuota, handleTranscribe } from "./transcribe";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const { pathname } = url;
    const { method } = request;

    const origin = request.headers.get("Origin") ?? "";
    const cors = corsHeaders(origin);

    // CORS preflight (Tauri webview will send these)
    if (method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    let response: Response;

    if (method === "POST" && pathname === "/transcribe") {
      response = await handleTranscribe(request, env);
    } else if (method === "GET" && pathname === "/quota") {
      response = await handleQuota(request, env);
    } else if (method === "GET" && pathname === "/health") {
      response = new Response("ok", { status: 200 });
    } else {
      response = new Response(JSON.stringify({ error: "Not Found" }), {
        status: 404,
        headers: { "Content-Type": "application/json" },
      });
    }

    // Attach CORS headers to all responses
    for (const [k, v] of Object.entries(cors)) {
      response.headers.set(k, v);
    }
    return response;
  },
} satisfies ExportedHandler<Env>;

/** Origins allowed to call this worker (Tauri webview on each platform). */
const ALLOWED_ORIGINS = new Set([
  "tauri://localhost",        // Windows / Linux
  "http://tauri.localhost",   // macOS
]);

function corsHeaders(origin: string): Record<string, string> {
  return {
    "Access-Control-Allow-Origin": ALLOWED_ORIGINS.has(origin) ? origin : "",
    "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    "Access-Control-Allow-Headers":
      "Content-Type, X-Device-Hash, Authorization",
    "Access-Control-Max-Age": "86400",
    "Vary": "Origin",
  };
}

/**
 * Deepgram Nova-3 transcription client.
 * Used as fallback if Groq Hebrew quality fails the Phase 0 benchmark gate.
 */

const DEEPGRAM_URL =
  "https://api.deepgram.com/v1/listen?model=nova-3&language=he&smart_format=true";

interface DeepgramResponse {
  results: {
    channels: Array<{
      alternatives: Array<{ transcript: string }>;
    }>;
  };
}

export async function transcribeDeepgram(
  audio: ArrayBuffer,
  apiKey: string
): Promise<string> {
  const response = await fetch(DEEPGRAM_URL, {
    method: "POST",
    headers: {
      Authorization: `Token ${apiKey}`,
      "Content-Type": "audio/wav",
    },
    body: audio,
  });

  if (!response.ok) {
    // Log full error server-side; never expose backend details to callers.
    const errText = await response.text();
    console.error(`Deepgram ${response.status}: ${errText.slice(0, 500)}`);
    throw new Error("Transcription service temporarily unavailable");
  }

  const json = (await response.json()) as DeepgramResponse;
  const transcript = json.results?.channels?.[0]?.alternatives?.[0]?.transcript;
  if (typeof transcript !== "string") {
    throw new Error("Deepgram response missing transcript");
  }
  return transcript.trim();
}

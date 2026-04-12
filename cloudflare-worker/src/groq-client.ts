/**
 * Groq Whisper transcription client.
 * Default model: whisper-large-v3-turbo (set in wrangler.toml GROQ_MODEL var).
 */

const GROQ_TRANSCRIBE_URL = "https://api.groq.com/openai/v1/audio/transcriptions";

export async function transcribeGroq(
  audio: ArrayBuffer,
  apiKey: string,
  model: string
): Promise<string> {
  const formData = new FormData();
  formData.append("file", new Blob([audio], { type: "audio/wav" }), "audio.wav");
  formData.append("model", model);
  formData.append("language", "he");
  formData.append("response_format", "json");

  const response = await fetch(GROQ_TRANSCRIBE_URL, {
    method: "POST",
    headers: { Authorization: `Bearer ${apiKey}` },
    body: formData,
  });

  if (!response.ok) {
    const errText = await response.text();
    throw new Error(`Groq ${response.status}: ${errText.slice(0, 200)}`);
  }

  const json = (await response.json()) as { text?: string };
  if (typeof json.text !== "string") {
    throw new Error("Groq response missing 'text' field");
  }
  return json.text.trim();
}

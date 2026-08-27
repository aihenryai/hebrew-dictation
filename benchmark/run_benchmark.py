#!/usr/bin/env python3
"""
Hebrew Dictation — Backend Benchmark Harness (Phase 0 Gate)

Compares Groq whisper-large-v3-turbo vs Deepgram Nova-3 (batch AND streaming) vs
Deepgram Nova-3 + keyterm prompting vs local faster-whisper (ivrit.ai or generic)
on a set of Hebrew audio samples with ground-truth reference transcripts.

Outputs a Markdown report with WER per backend + per sample, and a decision gate.

Deepgram streaming vs batch matters here: streaming commits to a word with less
right-hand context, so it is a DIFFERENT engine for WER purposes even though it's
the same underlying model. If the app's default (streaming) is the one producing
the errors a user reports, benchmarking only batch would hide that. See
docs/research/2026-08-27-vocabulary-mechanisms.md for why keyterm is included and
what it can and can't fix.

Sample audio should be 16kHz mono 16-bit PCM WAV — exactly what the app's
`debug_save_audio` diagnostic setting writes (see src-tauri/src/lib.rs,
`save_debug_sample`), so samples captured from a real dictation drop in here
with no conversion step.
"""

import argparse
import json
import os
import sys
import time
import wave
from pathlib import Path

import requests
from dotenv import load_dotenv

try:
    from websocket import create_connection, WebSocketTimeoutException  # websocket-client
    HAS_WEBSOCKET_CLIENT = True
except ImportError:
    HAS_WEBSOCKET_CLIENT = False

try:
    from jiwer import wer, Compose, RemovePunctuation, Strip, RemoveMultipleSpaces
    HAS_JIWER = True
except ImportError:
    HAS_JIWER = False

try:
    from faster_whisper import WhisperModel  # type: ignore
    HAS_FASTER_WHISPER = True
except ImportError:
    HAS_FASTER_WHISPER = False


# Hebrew-friendly WER: we strip punctuation + normalize whitespace but keep casing (Hebrew has no case)
def hebrew_wer(reference: str, hypothesis: str) -> float:
    transformation = Compose([RemovePunctuation(), Strip(), RemoveMultipleSpaces()])
    return wer(reference, hypothesis, truth_transform=transformation, hypothesis_transform=transformation)


def transcribe_groq(audio_path: Path, api_key: str, model: str = "whisper-large-v3-turbo"):
    url = "https://api.groq.com/openai/v1/audio/transcriptions"
    headers = {"Authorization": f"Bearer {api_key}"}
    start = time.time()
    with open(audio_path, "rb") as f:
        files = {"file": (audio_path.name, f, "audio/wav")}
        data = {"model": model, "language": "he", "response_format": "json"}
        response = requests.post(url, headers=headers, files=files, data=data, timeout=120)
    elapsed = time.time() - start
    response.raise_for_status()
    return response.json()["text"].strip(), elapsed


def transcribe_deepgram(audio_path: Path, api_key: str, keyterms=None):
    """Batch Deepgram Nova-3. `keyterms`, if given, adds repeated `&keyterm=`
    params (Keyterm Prompting — supported for Hebrew on Nova-3 since Feb 2026,
    see docs/research/2026-08-27-vocabulary-mechanisms.md). A bad keyterm is
    silently ignored by the API rather than erroring, so this is safe to try."""
    url = "https://api.deepgram.com/v1/listen?model=nova-3&language=he&smart_format=true"
    for term in (keyterms or []):
        url += f"&keyterm={requests.utils.quote(term)}"
    headers = {
        "Authorization": f"Token {api_key}",
        "Content-Type": "audio/wav",
    }
    start = time.time()
    with open(audio_path, "rb") as f:
        response = requests.post(url, headers=headers, data=f.read(), timeout=120)
    elapsed = time.time() - start
    response.raise_for_status()
    result = response.json()
    text = result["results"]["channels"][0]["alternatives"][0]["transcript"].strip()
    return text, elapsed


def _read_pcm16_mono_16k(audio_path: Path) -> bytes:
    """Read a WAV file's raw PCM16 frames, requiring 16kHz mono — exactly what
    Deepgram streaming's `encoding=linear16&sample_rate=16000&channels=1` expects
    and exactly what the app's `debug_save_audio` diagnostic writes. Refuses to
    silently resample (that would benchmark a different signal than what the app
    actually sends) — convert with ffmpeg instead:
    `ffmpeg -i in.wav -ar 16000 -ac 1 -sample_fmt s16 out.wav`."""
    with wave.open(str(audio_path), "rb") as wf:
        if wf.getframerate() != 16000 or wf.getnchannels() != 1 or wf.getsampwidth() != 2:
            raise ValueError(
                f"{audio_path.name} is {wf.getframerate()}Hz/{wf.getnchannels()}ch/"
                f"{wf.getsampwidth() * 8}bit — streaming benchmark needs 16kHz mono 16-bit PCM. "
                f"Convert: ffmpeg -i {audio_path} -ar 16000 -ac 1 -sample_fmt s16 out.wav"
            )
        return wf.readframes(wf.getnframes())


def transcribe_deepgram_streaming(audio_path: Path, api_key: str, chunk_ms: int = 100):
    """Deepgram Nova-3 over the streaming WebSocket — the SAME path the app uses
    by default (streaming_enabled: true), not the batch endpoint above. Streaming
    finalizes each segment with less right-hand context than a batch pass over the
    whole utterance, so this is a materially different measurement from
    `transcribe_deepgram`, mirroring src-tauri/src/streaming.rs: accumulate
    `is_final` transcript segments, send CloseStream, drain remaining messages.
    Chunks are sent back-to-back (no real-time pacing) — fine for a WER
    measurement, since Deepgram finalizes on speech pauses in the audio itself,
    not on wall-clock arrival time."""
    if not HAS_WEBSOCKET_CLIENT:
        raise RuntimeError("websocket-client not installed. Run: pip install websocket-client")

    pcm = _read_pcm16_mono_16k(audio_path)
    bytes_per_chunk = int(16000 * 2 * (chunk_ms / 1000.0))  # 16kHz * 2 bytes/sample * chunk_ms

    url = (
        "wss://api.deepgram.com/v1/listen?model=nova-3&language=he"
        "&encoding=linear16&sample_rate=16000&channels=1"
        "&smart_format=true&punctuate=true&interim_results=true"
    )
    start = time.time()
    ws = create_connection(url, header=[f"Authorization: Token {api_key}"], timeout=30)
    try:
        for i in range(0, len(pcm), bytes_per_chunk):
            ws.send_binary(pcm[i:i + bytes_per_chunk])
        ws.send(json.dumps({"type": "CloseStream"}))

        segments = []
        while True:
            try:
                raw = ws.recv()
            except WebSocketTimeoutException:
                break
            if not raw:
                break
            try:
                msg = json.loads(raw)
            except (json.JSONDecodeError, TypeError):
                continue
            if msg.get("type") == "Metadata":
                break  # Deepgram sends a final Metadata message once CloseStream is processed
            transcript = msg.get("channel", {}).get("alternatives", [{}])[0].get("transcript", "")
            if transcript and msg.get("is_final"):
                segments.append(transcript)
    finally:
        ws.close()
    elapsed = time.time() - start
    return " ".join(segments).strip(), elapsed


def transcribe_local(audio_path: Path, model):
    start = time.time()
    segments, _ = model.transcribe(str(audio_path), language="he", beam_size=1)
    text = " ".join(seg.text for seg in segments).strip()
    elapsed = time.time() - start
    return text, elapsed


def load_samples(samples_dir: Path):
    samples = []
    if not samples_dir.exists():
        return samples
    for sample_dir in sorted(samples_dir.iterdir()):
        if not sample_dir.is_dir():
            continue
        audio_files = []
        for ext in ("wav", "mp3", "m4a", "ogg", "flac"):
            audio_files.extend(sample_dir.glob(f"audio.{ext}"))
        ref_file = sample_dir / "reference.txt"
        if not audio_files or not ref_file.exists():
            print(f"warn: skipping {sample_dir.name} (missing audio or reference.txt)", file=sys.stderr)
            continue
        samples.append({
            "name": sample_dir.name,
            "audio": audio_files[0],
            "reference": ref_file.read_text(encoding="utf-8").strip(),
        })
    return samples


def main():
    parser = argparse.ArgumentParser(description="Hebrew transcription backend benchmark (Phase 0 gate)")
    parser.add_argument("--samples", type=Path, default=Path(__file__).parent / "samples")
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / "results.md")
    parser.add_argument("--skip-groq", action="store_true")
    parser.add_argument("--skip-deepgram", action="store_true")
    parser.add_argument("--skip-deepgram-streaming", action="store_true",
                        help="Skip the WebSocket-streaming backend (needs `pip install websocket-client`)")
    parser.add_argument("--skip-local", action="store_true")
    parser.add_argument("--local-model", default="large-v3-turbo",
                        help="For Hebrew, prefer ivrit-ai/whisper-large-v3-turbo-ct2 "
                             "(faster-whisper/CTranslate2 build; not the ggml build the app uses)")
    parser.add_argument("--groq-model", default="whisper-large-v3-turbo",
                        help="Fallback: whisper-large-v3 (non-turbo) if turbo WER too high")
    parser.add_argument("--keyterms", default="",
                        help="Comma-separated terms for Deepgram Keyterm Prompting — adds a "
                             "'Deepgram Nova-3 + keyterm' backend. See "
                             "docs/research/2026-08-27-vocabulary-mechanisms.md before choosing terms.")
    args = parser.parse_args()

    load_dotenv(Path(__file__).parent / ".env")

    if not HAS_JIWER:
        print("error: jiwer not installed. Run: pip install -r requirements.txt", file=sys.stderr)
        sys.exit(1)

    samples = load_samples(args.samples)
    if not samples:
        print(f"error: no valid samples in {args.samples}", file=sys.stderr)
        print("   expected: samples/sample_NN/{audio.wav,reference.txt}", file=sys.stderr)
        sys.exit(1)

    print(f"loaded {len(samples)} samples")

    groq_key = os.getenv("GROQ_API_KEY") if not args.skip_groq else None
    deepgram_key = os.getenv("DEEPGRAM_API_KEY") if not args.skip_deepgram else None

    local_model = None
    if not args.skip_local:
        if HAS_FASTER_WHISPER:
            print(f"loading local model {args.local_model}...")
            local_model = WhisperModel(args.local_model, device="cpu", compute_type="int8")
            print("  loaded")
        else:
            print("warn: faster-whisper not installed, skipping local", file=sys.stderr)

    keyterms = [t.strip() for t in args.keyterms.split(",") if t.strip()]

    backends = []
    if groq_key:
        backends.append((
            f"Groq {args.groq_model}",
            lambda a: transcribe_groq(a, groq_key, args.groq_model),
        ))
    if deepgram_key:
        backends.append(("Deepgram Nova-3 (batch)", lambda a: transcribe_deepgram(a, deepgram_key)))
        if not args.skip_deepgram_streaming:
            if HAS_WEBSOCKET_CLIENT:
                backends.append((
                    "Deepgram Nova-3 (streaming)",
                    lambda a: transcribe_deepgram_streaming(a, deepgram_key),
                ))
            else:
                print("warn: websocket-client not installed, skipping streaming backend "
                      "(pip install websocket-client)", file=sys.stderr)
        if keyterms:
            backends.append((
                "Deepgram Nova-3 (batch + keyterm)",
                lambda a: transcribe_deepgram(a, deepgram_key, keyterms=keyterms),
            ))
    if local_model is not None:
        backends.append((f"Local faster-whisper {args.local_model}", lambda a: transcribe_local(a, local_model)))

    if not backends:
        print("error: no backends enabled (check API keys / --skip flags)", file=sys.stderr)
        sys.exit(1)

    # Run benchmarks
    results = {}
    for backend_name, transcribe_fn in backends:
        print(f"\n>> {backend_name}")
        results[backend_name] = []
        for sample in samples:
            try:
                text, elapsed = transcribe_fn(sample["audio"])
                w = hebrew_wer(sample["reference"], text)
                print(f"   {sample['name']}: WER={w:.2%} ({elapsed:.1f}s)")
                results[backend_name].append({
                    "sample": sample["name"],
                    "text": text,
                    "elapsed": elapsed,
                    "wer": w,
                    "error": None,
                })
            except Exception as e:
                print(f"   ERROR {sample['name']}: {e}", file=sys.stderr)
                results[backend_name].append({
                    "sample": sample["name"],
                    "text": None,
                    "elapsed": 0.0,
                    "wer": None,
                    "error": str(e),
                })

    # Write markdown report
    with open(args.output, "w", encoding="utf-8") as f:
        f.write("# Hebrew Dictation — Backend Benchmark Results\n\n")
        f.write(f"Samples: **{len(samples)}** | Backends: **{len(backends)}**\n\n")
        f.write("## Summary (average WER per backend)\n\n")
        f.write("| Backend | Avg WER | Avg latency (s) | Valid samples |\n")
        f.write("|---|---|---|---|\n")
        for backend_name, runs in results.items():
            valid = [r for r in runs if r["wer"] is not None]
            if not valid:
                f.write(f"| {backend_name} | (all errors) | — | 0/{len(runs)} |\n")
                continue
            avg_wer = sum(r["wer"] for r in valid) / len(valid)
            avg_latency = sum(r["elapsed"] for r in valid) / len(valid)
            f.write(f"| {backend_name} | {avg_wer:.2%} | {avg_latency:.2f} | {len(valid)}/{len(runs)} |\n")

        f.write("\n## Per-sample detail\n\n")
        for sample in samples:
            f.write(f"### {sample['name']}\n")
            f.write(f"**Reference:** `{sample['reference']}`\n\n")
            for backend_name, runs in results.items():
                run = next((r for r in runs if r["sample"] == sample["name"]), None)
                if run is None:
                    continue
                if run["error"]:
                    f.write(f"- **{backend_name}**: ERROR `{run['error']}`\n")
                else:
                    f.write(f"- **{backend_name}** (WER {run['wer']:.2%}, {run['elapsed']:.1f}s): `{run['text']}`\n")
            f.write("\n")

    print(f"\nresults written to {args.output}")

    # Decision gate — key output
    groq_key_label = f"Groq {args.groq_model}"
    groq_runs = [r for r in results.get(groq_key_label, []) if r["wer"] is not None]
    if groq_runs:
        groq_avg = sum(r["wer"] for r in groq_runs) / len(groq_runs)
        print(f"\n=== DECISION GATE (Groq avg WER: {groq_avg:.2%}) ===")
        if groq_avg < 0.15:
            print("  PROCEED with Groq backend for Phase 1 Worker")
        elif groq_avg < 0.25:
            print("  RETRY with whisper-large-v3 (non-turbo): --groq-model whisper-large-v3")
        else:
            print("  FALLBACK to Deepgram (update margin calc in plan)")


if __name__ == "__main__":
    main()

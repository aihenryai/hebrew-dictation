#!/usr/bin/env python3
"""
Hebrew Dictation — Backend Benchmark Harness (Phase 0 Gate)

Compares Groq whisper-large-v3-turbo vs Deepgram Nova-3 vs local faster-whisper
on a set of Hebrew audio samples with ground-truth reference transcripts.

Outputs a Markdown report with WER per backend + per sample, and a decision gate.
"""

import argparse
import os
import sys
import time
from pathlib import Path

import requests
from dotenv import load_dotenv

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


def transcribe_deepgram(audio_path: Path, api_key: str):
    url = "https://api.deepgram.com/v1/listen?model=nova-3&language=he&smart_format=true"
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
    parser.add_argument("--skip-local", action="store_true")
    parser.add_argument("--local-model", default="large-v3-turbo")
    parser.add_argument("--groq-model", default="whisper-large-v3-turbo",
                        help="Fallback: whisper-large-v3 (non-turbo) if turbo WER too high")
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

    backends = []
    if groq_key:
        backends.append((
            f"Groq {args.groq_model}",
            lambda a: transcribe_groq(a, groq_key, args.groq_model),
        ))
    if deepgram_key:
        backends.append(("Deepgram Nova-3", lambda a: transcribe_deepgram(a, deepgram_key)))
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

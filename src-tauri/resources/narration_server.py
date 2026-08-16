"""Hebrew narration sidecar: text -> diacritics+stress -> IPA -> VITS ONNX -> WAV.

Replaces `python -m piper.http_server`. Piper's server phonemizes internally
from nikud, which encodes vowels but NOT stress — the cause of the flat,
run-together delivery. The voices this serves are trained on Phonikud's
stress-marked IPA and declare `phoneme_type: "raw"`, so phonemization must
happen here, before inference.

Deliberately exposes the SAME HTTP surface piper's server did (GET /info,
POST /synthesize returning raw WAV bytes) so the Rust client is unchanged.

Runs fully offline: the tokenizer is loaded from a local file rather than
the HuggingFace Hub, which phonikud_onnx would otherwise reach for on every
construction.
"""

import argparse
import io
import json
import re
import sys
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import numpy as np
import onnxruntime as ort


def build_phonemizer(phonikud_model_path: str, tokenizer_path: str):
    """Load Phonikud with a local tokenizer.

    phonikud_onnx hardcodes `Tokenizer.from_pretrained("dicta-il/...")`, which
    hits the network at construction time and offers no local-path option.
    Redirecting the loader is what keeps first synthesis working with no
    internet — the file itself is downloaded once during provisioning.
    """
    from tokenizers import Tokenizer
    import phonikud_onnx.model as phonikud_model_module

    phonikud_model_module.Tokenizer.from_pretrained = staticmethod(
        lambda _name: Tokenizer.from_file(tokenizer_path)
    )

    from phonikud_onnx import Phonikud

    return Phonikud(phonikud_model_path)


# Sentence terminators, kept explicit so the silence insertion below is
# predictable rather than dependent on a locale-aware splitter.
_SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+")

# Hard caps on request input. The Rust client already clamps every one of
# these, but this server listens on a real TCP port that any local process --
# including a web page doing fetch('http://127.0.0.1:5758/...') -- can reach,
# so it cannot assume its caller validated anything. Measured before adding
# these: sentence_silence=1e9 made numpy attempt a 40 TiB allocation,
# a negative value raised on negative dimensions, and length_scale=0 returned
# HTTP 200 with 0.01s of garbage.
MAX_BODY_BYTES = 1_000_000  # ~1MB of JSON is far more text than is sensible
LIMITS = {
    # name: (low, high, default_attr_or_none)
    "length_scale": (0.8, 1.8),
    "sentence_silence": (0.0, 2.0),
    "noise_scale": (0.0, 1.2),
    "noise_w": (0.0, 1.2),
}


def clamp_param(name: str, raw, fallback: float) -> float:
    """Coerce one request parameter into a range that still produces speech.

    Falls back rather than erroring on junk: a bad tuning value should not cost
    the caller their synthesis, and the ranges here mirror
    NarrationParams::clamped on the Rust side.
    """
    low, high = LIMITS[name]
    try:
        value = float(raw)
    except (TypeError, ValueError):
        return fallback
    if value != value or value in (float("inf"), float("-inf")):  # NaN / inf
        return fallback
    return max(low, min(high, value))


def split_sentences(text: str):
    return [s for s in (p.strip() for p in _SENTENCE_SPLIT.split(text)) if s]


class Synthesizer:
    def __init__(self, voice_path, config_path, phonikud_path, tokenizer_path):
        self.config = json.loads(Path(config_path).read_text(encoding="utf-8"))
        self.phoneme_id_map = self.config["phoneme_id_map"]
        self.sample_rate = self.config["audio"]["sample_rate"]
        inference = self.config.get("inference", {})
        self.noise_scale = float(inference.get("noise_scale", 0.667))
        self.noise_w = float(inference.get("noise_w", 0.8))
        self.num_speakers = int(self.config.get("num_speakers", 1))

        self.session = ort.InferenceSession(
            voice_path, providers=["CPUExecutionProvider"]
        )
        self.phonikud = build_phonemizer(phonikud_path, tokenizer_path)
        import phonikud

        self._phonemize = phonikud.phonemize

    def to_ids(self, phonemes: str):
        """Phoneme string -> VITS symbol ids, in piper's BOS/PAD/EOS layout.

        Unknown symbols are skipped rather than raising: a stray character
        must degrade one word, never fail the whole request.
        """
        ids = list(self.phoneme_id_map["^"])
        for ch in phonemes:
            mapped = self.phoneme_id_map.get(ch)
            if mapped is None:
                continue
            ids.extend(mapped)
            ids.extend(self.phoneme_id_map["_"])
        ids.extend(self.phoneme_id_map["$"])
        return ids

    def synthesize_sentence(
        self, sentence: str, length_scale: float, noise_scale: float, noise_w: float
    ):
        with_diacritics = self.phonikud.add_diacritics(sentence)
        ipa = self._phonemize(with_diacritics)
        ids = self.to_ids(ipa)
        inputs = {
            "input": np.array([ids], dtype=np.int64),
            "input_lengths": np.array([len(ids)], dtype=np.int64),
            "scales": np.array(
                [noise_scale, length_scale, noise_w], dtype=np.float32
            ),
        }
        if self.num_speakers > 1:
            inputs["sid"] = np.array([0], dtype=np.int64)
        audio = self.session.run(None, inputs)[0].squeeze()
        # `.max()` raises on a zero-size array, so check size before peak:
        # input that maps to no known phonemes can yield an empty result.
        if audio.size == 0:
            return np.zeros(0, dtype=np.int16)
        peak = float(np.abs(audio).max())
        if peak > 0:
            audio = audio / peak * 0.95
        return (audio * 32767).astype(np.int16)

    def synthesize(
        self,
        text: str,
        length_scale: float,
        sentence_silence: float,
        noise_scale: float,
        noise_w: float,
    ):
        sentences = split_sentences(text) or [text]
        silence = np.zeros(int(self.sample_rate * sentence_silence), dtype=np.int16)
        chunks = []
        for i, sentence in enumerate(sentences):
            if i > 0 and silence.size:
                chunks.append(silence)
            chunks.append(
                self.synthesize_sentence(sentence, length_scale, noise_scale, noise_w)
            )
        pcm = np.concatenate(chunks) if chunks else np.zeros(0, dtype=np.int16)

        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(self.sample_rate)
            w.writeframes(pcm.tobytes())
        return buf.getvalue()


def make_handler(synth: Synthesizer, args):
    class Handler(BaseHTTPRequestHandler):
        # Silence per-request logging; stdout/stderr are piped to null by the
        # parent process anyway, and a blocked pipe would stall synthesis.
        def log_message(self, *_args):
            pass

        def do_GET(self):
            if self.path.split("?")[0] != "/info":
                self.send_error(404)
                return
            body = json.dumps(
                {
                    "voice": {
                        "name": Path(args.model).stem,
                        "language": "he",
                        "num_speakers": synth.num_speakers,
                    },
                    "engine": "phonikud",
                }
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path.split("?")[0] != "/synthesize":
                self.send_error(404)
                return
            try:
                length = int(self.headers.get("Content-Length", 0))
                # Refuse an oversized body BEFORE reading it, so a bogus
                # Content-Length cannot make us allocate on a caller's say-so.
                if length > MAX_BODY_BYTES:
                    self.send_error(413, "request body too large")
                    return
                payload = json.loads(self.rfile.read(length).decode("utf-8"))
                if not isinstance(payload, dict):
                    self.send_error(400, "body must be a JSON object")
                    return
                text = (payload.get("text") or "").strip()
                if not text:
                    self.send_error(400, "empty text")
                    return
                # Every knob is per-request with the startup flag as fallback,
                # and every one is clamped — see LIMITS for why.
                length_scale = clamp_param(
                    "length_scale", payload.get("length_scale"), args.length_scale
                )
                sentence_silence = clamp_param(
                    "sentence_silence",
                    payload.get("sentence_silence"),
                    args.sentence_silence,
                )
                noise_scale = clamp_param(
                    "noise_scale", payload.get("noise_scale"), synth.noise_scale
                )
                noise_w = clamp_param("noise_w", payload.get("noise_w"), synth.noise_w)
                wav = synth.synthesize(
                    text, length_scale, sentence_silence, noise_scale, noise_w
                )
            except Exception as exc:  # noqa: BLE001 - report, never kill the server
                self.send_error(500, str(exc)[:200])
                return
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(wav)))
            self.end_headers()
            self.wfile.write(wav)

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--phonikud", required=True)
    parser.add_argument("--tokenizer", required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=5758)
    parser.add_argument("--sentence-silence", type=float, default=0.45)
    parser.add_argument("--length-scale", type=float, default=1.0)
    args = parser.parse_args()

    synth = Synthesizer(args.model, args.config, args.phonikud, args.tokenizer)
    server = ThreadingHTTPServer((args.host, args.port), make_handler(synth, args))
    # Only printed once the voice and diacritizer are actually loaded, so the
    # parent's readiness poll can't see the port open before it can serve.
    print(f"narration_server: listening on http://{args.host}:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    sys.exit(main())

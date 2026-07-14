import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import "./App.css";

/* ----------- אפליקציה: קבועים ----------- */
const APP_VERSION = "v2.11.0";
const APP_LICENSE = "MIT";

// System-audio capture (WASAPI loopback) is Windows-only, so System/Call are
// hidden off-Windows — non-Windows users only ever see Mic (zero regression).
// WebView2 on Windows always reports "Windows" in navigator.userAgent.
const IS_WINDOWS =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

/** Hebrew label for a batch-transcription progress stage. */
function stageLabel(stage: string): string {
  switch (stage) {
    case "decoding": return "מפענח אודיו…";
    case "uploading": return "מעלה…";
    case "transcribing": return "מתמלל…";
    case "done": return "הושלם";
    default: return "מעבד…";
  }
}

const TERMS_FULL_URL = "https://henry-ai-website.pages.dev/hebrew-dictation#terms";

const LINKS = {
  youtube: "https://youtube.com/@AIWithHenry",
  whatsappChannel: "https://chat.whatsapp.com/Kx8EwqLre3fBSfhbpFE2tF",
  taplink: "https://taplink.cc/henry.ai",
  github: "https://github.com/aihenryai/hebrew-dictation",
  email: "henrystauber22@gmail.com",
};

// פידבק והצעות לשיפור — נשלח ישירות לאימייל של הנרי
const FEEDBACK_URL = `mailto:${LINKS.email}?subject=${encodeURIComponent(
  "פידבק על הכתבה בעברית"
)}&body=${encodeURIComponent(
  "היי הנרי,\n\n[כתבו כאן: באג / בקשה לפיצ'ר / שאלה / מחשבה]\n\nגרסה: " +
    APP_VERSION +
    "\n"
)}`;

type AppStatus = "idle" | "recording" | "transcribing" | "enhancing" | "downloading" | "loading-model";
type AppView = "main" | "settings" | "onboarding" | "batch";
type BatchFileStatus = "pending" | "processing" | "done" | "cancelled" | "error";
interface TimedSegment {
  text: string;
  start_ms: number;
  end_ms: number;
  // Diarization / Call channel index (0 = "אני", else "הצד השני"). Carried opaquely
  // from the backend and sent back on export_srt; drives the Call SRT labels.
  speaker?: number | null;
}
interface BatchResult {
  id: number;
  fileName: string;
  filePath: string;
  transcript: string;
  status: BatchFileStatus;
  error?: string;
  segments?: TimedSegment[];
  /** True once the user hand-edits `transcript` in the textarea — segments
   * no longer match the (unedited) text, so SRT export is hidden for this item. */
  edited?: boolean;
  /** True for a Call recording — SRT export uses אני/הצד השני side labels instead
   * of the diarization דובר N: prefix (Task 20). Falsy/undefined for Mic/System. */
  isCallCloud?: boolean;
}
let batchIdCounter = 0;
type Language = "he" | "en" | "multi";
type TranscriptionMode = "api" | "local" | "auto_fallback";
type ApiProvider = "deepgram" | "groq";

/** Recording input source, chosen before a batch recording and threaded into
 * `start_batch_recording` (Task 18). Default "mic" = existing behavior (zero
 * regression). "system" (WASAPI loopback), "callcloud" (mic + system → cloud
 * multichannel) and "calllocal" (mic + system → mixed mono, local whisper) are
 * Windows-only — see IS_WINDOWS gating in the batch view. Wire values are
 * lowercase to match the app's existing `mode`/`language` invoke payloads. */
type RecordingSource = "mic" | "system" | "callcloud" | "calllocal";

/** Settings sent to the backend. API keys are managed separately via set_api_key / clear_api_key. */
interface AppSettings {
  transcription_mode: TranscriptionMode;
  api_provider: ApiProvider;
  preferred_model: string;
  language: string;
  vad_enabled: boolean;
  onboarding_completed?: boolean;
  terms_accepted?: boolean;
  close_notification_shown?: boolean;
  always_on_top?: boolean;
  autostart_enabled?: boolean;
  streaming_enabled?: boolean;
  floating_toolbar_enabled?: boolean;
  hotkey?: string;
  pause_hotkey?: string | null;
  vad_silence_secs?: number;
  max_recording_secs?: number;
  unlimited_recording?: boolean;
  preferred_audio_device?: string | null;
  audio_feedback_enabled?: boolean;
  idle_button_enabled?: boolean;
  audio_volume?: number;
  enhance_enabled?: boolean;
  enhance_mode?: string;
}

/** Redacted settings returned from the backend (keys replaced with booleans). */
interface RedactedSettings {
  transcription_mode: TranscriptionMode;
  api_provider: ApiProvider;
  has_deepgram_key: boolean;
  has_groq_key: boolean;
  preferred_model: string;
  language: string;
  vad_enabled: boolean;
  onboarding_completed?: boolean;
  terms_accepted?: boolean;
  close_notification_shown?: boolean;
  always_on_top?: boolean;
  autostart_enabled?: boolean;
  streaming_enabled?: boolean;
  floating_toolbar_enabled?: boolean;
  hotkey?: string;
  pause_hotkey?: string | null;
  vad_silence_secs?: number;
  max_recording_secs?: number;
  unlimited_recording?: boolean;
  preferred_audio_device?: string | null;
  audio_feedback_enabled?: boolean;
  idle_button_enabled?: boolean;
  audio_volume?: number;
  enhance_enabled?: boolean;
  enhance_mode?: string;
}

/** VAD state payload pushed from the backend ~every 500ms while recording. */
interface VadStatePayload {
  state: "speaking" | "silent";
  silent_secs: number;
  silence_total: number;
  vad_off: boolean;
}

/** History item shape sent to backend `export_history`. Mirrors `export.rs::HistoryItem`. */
interface ExportHistoryItem {
  text: string;
  timestamp?: string;
}

/** First 4 words of a transcript, capped at 40 chars — used as a content-derived
 *  export filename for BOTH the regular dictation history and batch file results. */
function firstWordsName(text: string): string {
  const words = text.trim().split(/\s+/).slice(0, 4).join(" ");
  return words.length > 40 ? words.substring(0, 40) : words;
}

interface InterimPayload {
  text: string;
  is_final: boolean;
}

interface ModelInfo {
  name: string;
  size_bytes: number;
  size_label: string;
  downloaded: boolean;
  description: string;
}

const MIN_TRANSCRIBE_SAMPLES = 8000;
const MAX_RECORDING_LOCAL = 60;
const MAX_RECORDING_API = 120; // 2 minutes max for API

/** Human-readable label for a hotkey combo string (e.g. "alt+d" → "Alt + D"). */
function formatHotkey(combo: string): string {
  if (!combo) return "";
  return combo
    .split("+")
    .map((part) => {
      const trimmed = part.trim();
      if (!trimmed) return "";
      return trimmed.charAt(0).toUpperCase() + trimmed.slice(1).toLowerCase();
    })
    .filter(Boolean)
    .join(" + ");
}

/** Build a Tauri-compatible combo string from a keyboard event. Returns null
 * for modifier-only presses (no main key yet) — caller keeps capturing.
 *
 * Uses `event.code` (layout-independent physical key) rather than `event.key`
 * (Unicode character produced by the active layout). On Hebrew keyboard
 * layouts, pressing the physical "D" key gives `event.key === "ד"` which
 * Tauri's Shortcut::parse rejects. With `event.code === "KeyD"`, the combo is
 * always stored as "alt+d" regardless of which layout was active when the
 * user captured it — and the OS-level shortcut fires on the same physical
 * key whatever layout is active later. */
function buildComboFromKeyEvent(e: KeyboardEvent): string | null {
  const modifiers: string[] = [];
  if (e.ctrlKey) modifiers.push("ctrl");
  if (e.altKey) modifiers.push("alt");
  if (e.shiftKey) modifiers.push("shift");
  if (e.metaKey) modifiers.push("super");

  // Modifier-only press — keep capturing until a real key arrives.
  // event.key is fine for detecting modifier-only since it's locale-stable for these.
  if (["Control", "Alt", "Shift", "Meta", "Dead"].includes(e.key)) return null;

  const code = e.code;
  let mainKey: string | null = null;
  if (/^Key[A-Z]$/.test(code)) {
    mainKey = code.slice(3).toLowerCase(); // KeyD → "d"
  } else if (/^Digit\d$/.test(code)) {
    mainKey = code.slice(5); // Digit5 → "5"
  } else if (/^Numpad\d$/.test(code)) {
    mainKey = `num${code.slice(6)}`; // Numpad5 → "num5"
  } else if (/^F\d{1,2}$/.test(code)) {
    mainKey = code.toLowerCase(); // F8 → "f8"
  } else {
    const map: Record<string, string> = {
      ArrowUp: "up",
      ArrowDown: "down",
      ArrowLeft: "left",
      ArrowRight: "right",
      Enter: "enter",
      NumpadEnter: "enter",
      Escape: "escape",
      Backspace: "backspace",
      Tab: "tab",
      Space: "space",
      Insert: "insert",
      Delete: "delete",
      Home: "home",
      End: "end",
      PageUp: "pageup",
      PageDown: "pagedown",
      Minus: "minus",
      Equal: "equal",
      BracketLeft: "[",
      BracketRight: "]",
      Backslash: "\\",
      Semicolon: ";",
      Quote: "'",
      Comma: ",",
      Period: ".",
      Slash: "/",
    };
    mainKey = map[code] ?? null;
  }

  if (!mainKey) return null;

  // Reject combos without any modifier — too easy to trigger by accident
  // (typing "d" anywhere on the desktop). Unless it's an F-key.
  if (modifiers.length === 0 && !/^f\d{1,2}$/.test(mainKey)) return null;

  return [...modifiers, mainKey].join("+");
}

/** Render a duration in seconds as a Hebrew label: "30s", "2 דקות", "10 דקות". */
function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const mins = secs / 60;
  if (Number.isInteger(mins)) return mins === 1 ? "דקה" : `${mins} דקות`;
  return `${mins.toFixed(1)} דקות`;
}

/* ---------------------------------------------------------------------------
 * Audio feedback — short tones on record start/stop.
 * Sine wave, low volume (0.08), short duration (~80ms). Created via Web Audio
 * API so no asset files are needed. AudioContext is lazy: built on first use
 * and reused. The default browser autoplay policy permits AudioContext use
 * after any user-driven event, which the hotkey/button paths qualify as.
 * ---------------------------------------------------------------------------*/
let _audioCtx: AudioContext | null = null;
function getAudioCtx(): AudioContext | null {
  if (_audioCtx) return _audioCtx;
  try {
    const Ctor = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    if (!Ctor) return null;
    _audioCtx = new Ctor();
    return _audioCtx;
  } catch {
    return null;
  }
}

// Loudness for all feedback tones (0.0–1.0). Kept at module scope so the
// top-level play* helpers can read it; the React app syncs it from the
// `audio_volume` setting via `setToneVolume`.
let toneVolume = 0.6;
function setToneVolume(v: number) {
  toneVolume = Math.max(0, Math.min(1, v));
}

function playTone(frequency: number, durationSecs: number, volume?: number) {
  const ctx = getAudioCtx();
  if (!ctx) return;
  // Suspended (some Chromium policies) — try to resume; if it fails we skip.
  if (ctx.state === "suspended") {
    ctx.resume().catch(() => {});
  }
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = "sine";
  osc.frequency.value = frequency;
  // Gentle attack + release so it doesn't click. 0.08 is the reference peak
  // at full volume; the user's volume setting scales it down.
  const peak = 0.08 * (volume ?? toneVolume);
  const now = ctx.currentTime;
  gain.gain.setValueAtTime(0, now);
  gain.gain.linearRampToValueAtTime(peak, now + 0.01);
  gain.gain.linearRampToValueAtTime(0, now + durationSecs);
  osc.connect(gain);
  gain.connect(ctx.destination);
  osc.start(now);
  osc.stop(now + durationSecs + 0.02);
}

/** Two-tone arpeggio rising — "recording started". */
function playStartTone() {
  playTone(660, 0.07);          // E5
  setTimeout(() => playTone(880, 0.09), 70); // A5
}

/** Low descending two-tone — "something went wrong" (transcription error). */
function playErrorTone() {
  playTone(330, 0.10);          // E4
  setTimeout(() => playTone(220, 0.16), 90); // A3
}

/** Single soft high blip — "copied to clipboard". */
function playCopyTone() {
  playTone(990, 0.045);
}

/** Two-tone arpeggio falling — "recording stopped". */
function playStopTone() {
  playTone(880, 0.07);          // A5
  setTimeout(() => playTone(550, 0.10), 70); // C#5
}

let historyIdCounter = 0;
function App() {
  const [status, setStatus] = useState<AppStatus>("idle");
  const [view, setView] = useState<AppView>("main");
  // Which view "חזור" from settings returns to — so settings opened from the batch
  // screen returns there, not to main. Each settings entry point sets it.
  const [settingsReturn, setSettingsReturn] = useState<AppView>("main");
  const [transcript, setTranscript] = useState("");
  const [editableTranscript, setEditableTranscript] = useState("");
  const [history, setHistory] = useState<{ id: number; text: string; timestamp: string }[]>([]);
  const [whisperLoaded, setWhisperLoaded] = useState(false);
  // Background (startup) model load — must NOT block the dictation flow. Using the
  // global "loading-model" status froze Alt+D and the record button for the entire
  // load of a large local model (e.g. the 1.6GB ivrit model), so the floating bar
  // couldn't appear right after launch. This flag drives a non-blocking indicator.
  const [modelLoading, setModelLoading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [downloadingModel, setDownloadingModel] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [devices, setDevices] = useState<string[]>([]);
  const [selectedModel, setSelectedModel] = useState("small");
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [language, setLanguage] = useState<Language>("he");
  const [vadEnabled, setVadEnabled] = useState(true);
  const [recordingTime, setRecordingTime] = useState(0);
  const [transcriptionMode, setTranscriptionMode] = useState<TranscriptionMode>("auto_fallback");
  const [apiProvider, setApiProvider] = useState<ApiProvider>("deepgram");
  const [deepgramKey, setDeepgramKey] = useState("");
  const [groqKey, setGroqKey] = useState("");
  const [apiKeyValid, setApiKeyValid] = useState<boolean | null>(null);
  const [testingApiKey, setTestingApiKey] = useState(false);
  // Dedicated Groq-key field for Smart Cleanup (independent of the transcription provider).
  const [groqCleanupValid, setGroqCleanupValid] = useState<boolean | null>(null);
  const [testingGroqCleanup, setTestingGroqCleanup] = useState(false);
  const [wizardStep, setWizardStep] = useState(1);
  const [wizardApiKey, setWizardApiKey] = useState("");
  const [wizardKeyValid, setWizardKeyValid] = useState<boolean | null>(null);
  const [wizardKeyTesting, setWizardKeyTesting] = useState(false);
  const [wizardChoice, setWizardChoice] = useState<"api" | "groq" | "local" | null>(null);
  const [wizardProviderKey, setWizardProviderKey] = useState<"deepgram" | "groq">("deepgram");
  const [wizardTermsAsIs, setWizardTermsAsIs] = useState(false);
  const [wizardTermsKeys, setWizardTermsKeys] = useState(false);
  const [showTermsGate, setShowTermsGate] = useState(false);
  const [showCloseTip, setShowCloseTip] = useState(false);
  const [alwaysOnTop, setAlwaysOnTop] = useState(true);
  const [autostartEnabled, setAutostartEnabled] = useState(true);
  const [streamingEnabled, setStreamingEnabled] = useState(false);
  const [floatingToolbarEnabled, setFloatingToolbarEnabled] = useState(true);
  // v2.7.0 — configurable behavior
  const [hotkey, setHotkey] = useState<string>("alt+d");
  const [hotkeyCapturing, setHotkeyCapturing] = useState(false);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  // v2.8.0 — separate Pause/Resume hotkey, history export.
  const [pauseHotkey, setPauseHotkey] = useState<string | null>("alt+p");
  const [pauseHotkeyCapturing, setPauseHotkeyCapturing] = useState(false);
  const [pauseHotkeyError, setPauseHotkeyError] = useState<string | null>(null);
  const [exporting, setExporting] = useState<"txt" | "docx" | null>(null);
  const [exportNotice, setExportNotice] = useState<string | null>(null);
  const [vadSilenceSecs, setVadSilenceSecs] = useState<number>(4.5);
  const [maxRecordingSecs, setMaxRecordingSecs] = useState<number>(60);
  const [unlimitedRecording, setUnlimitedRecording] = useState<boolean>(false);
  const [preferredAudioDevice, setPreferredAudioDevice] = useState<string | null>(null);
  // v2.8.1 — short tone on record start/stop so user gets feedback even when
  // their target app obscures the floating toolbar.
  const [audioFeedbackEnabled, setAudioFeedbackEnabled] = useState<boolean>(true);
  const [enhanceEnabled, setEnhanceEnabled] = useState<boolean>(false);
  const [hasGroqKey, setHasGroqKey] = useState<boolean>(false);
  // v2.8.1 — always-floating idle button (discoverability) + tone loudness.
  const [idleButtonEnabled, setIdleButtonEnabled] = useState<boolean>(false);
  const [audioVolume, setAudioVolume] = useState<number>(0.6);
  const [livePreview, setLivePreview] = useState("");
  const [copiedHistoryId, setCopiedHistoryId] = useState<number | null>(null);
  const [updateAvailable, setUpdateAvailable] = useState<{ version: string } | null>(null);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateProgress, setUpdateProgress] = useState(0);
  // Batch file transcription (multi-file upload → cloud Deepgram / local whisper).
  const [batchResults, setBatchResults] = useState<BatchResult[]>([]);
  const [batchRunning, setBatchRunning] = useState(false);
  const [batchStage, setBatchStage] = useState("");
  const [batchPct, setBatchPct] = useState(0);
  const [batchMode, setBatchMode] = useState<"cloud" | "local">("cloud");
  const [batchError, setBatchError] = useState("");
  const [batchCurrentIdx, setBatchCurrentIdx] = useState(0);
  const [batchFileTotal, setBatchFileTotal] = useState(0);
  const batchCancelledRef = useRef(false);
  const [batchRecording, setBatchRecording] = useState(false);
  // Recording source for the batch record button. Default "mic" = zero regression.
  const [recordSource, setRecordSource] = useState<RecordingSource>("mic");
  const [batchRecordElapsed, setBatchRecordElapsed] = useState(0);
  const [batchActiveResultId, setBatchActiveResultId] = useState<number | null>(null);
  const batchRecordTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const batchRecordingRef = useRef(false);
  const updateRef = useRef<Update | null>(null);
  const pendingCloseTipRef = useRef(false);
  const statusRef = useRef(status);
  const vadEnabledRef = useRef(vadEnabled);
  const languageRef = useRef(language);
  const vadPollRef = useRef<number | null>(null);
  const timerRef = useRef<number | null>(null);
  const transcriptionModeRef = useRef(transcriptionMode);
  const streamingEnabledRef = useRef(streamingEnabled);
  const liveFinalRef = useRef("");

  useEffect(() => { statusRef.current = status; }, [status]);
  useEffect(() => { batchRecordingRef.current = batchRecording; }, [batchRecording]);
  useEffect(() => { vadEnabledRef.current = vadEnabled; }, [vadEnabled]);
  useEffect(() => { languageRef.current = language; }, [language]);
  useEffect(() => { transcriptionModeRef.current = transcriptionMode; }, [transcriptionMode]);
  useEffect(() => { streamingEnabledRef.current = streamingEnabled; }, [streamingEnabled]);
  const audioFeedbackEnabledRef = useRef(audioFeedbackEnabled);
  useEffect(() => { audioFeedbackEnabledRef.current = audioFeedbackEnabled; }, [audioFeedbackEnabled]);
  const enhanceEnabledRef = useRef(enhanceEnabled);
  useEffect(() => { enhanceEnabledRef.current = enhanceEnabled; }, [enhanceEnabled]);
  const audioVolumeRef = useRef(audioVolume);
  useEffect(() => { audioVolumeRef.current = audioVolume; setToneVolume(audioVolume); }, [audioVolume]);

  const maxRecordingSecsRef = useRef(60);
  const unlimitedRecordingRef = useRef(false);
  useEffect(() => { maxRecordingSecsRef.current = maxRecordingSecs; }, [maxRecordingSecs]);
  useEffect(() => { unlimitedRecordingRef.current = unlimitedRecording; }, [unlimitedRecording]);

  // User override → unlimited (mapped to backend's hard ceiling) → mode-based default.
  const getMaxRecordingSecs = () => {
    if (unlimitedRecordingRef.current) return 3600;
    if (maxRecordingSecsRef.current > 0) return maxRecordingSecsRef.current;
    return transcriptionModeRef.current === "local" ? MAX_RECORDING_LOCAL : MAX_RECORDING_API;
  };

  const stopVadPolling = useCallback(() => {
    if (vadPollRef.current) {
      clearInterval(vadPollRef.current);
      vadPollRef.current = null;
    }
  }, []);

  const stopTimer = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  // Inject text into the currently-focused text field (focus stays there — window never steals it)
  const injectText = useCallback(async (text: string) => {
    try {
      await invoke("inject_text", { text });
    } catch {
      // Injection may fail if no text field is focused
    }
  }, []);

  const stopAndTranscribe = useCallback(async (fromToolbar: boolean = false) => {
    if (statusRef.current !== "recording") return;

    setStatus("transcribing");
    stopVadPolling();
    stopTimer();
    // Audio feedback — short descending arpeggio when mic closes. Fires
    // BEFORE transcription so the user gets feedback immediately rather than
    // after the 1–5s transcribe wait.
    if (audioFeedbackEnabledRef.current) playStopTone();

    // Dismiss the floating bar IMMEDIATELY on stop — BEFORE transcription +
    // enhancement (which take a few seconds). Leaving it up makes it look like
    // recording is still running. forceShowMain only when the stop came from the
    // toolbar button (the user is looking at the app); on Alt+D from another app
    // we hide the bar without stealing focus. The main window's status carries
    // the wait either way.
    await invoke("hide_toolbar_window", { forceShowMain: fromToolbar }).catch(() => {});

    try {
      if (streamingEnabledRef.current) {
        // Streaming mode: each final segment was injected incrementally via
        // the streaming receive task (live dictation). The accumulated text is
        // returned here only for UI display (editable transcript + history).
        const text = await invoke("stop_streaming_transcription") as string;
        setLivePreview("");
        liveFinalRef.current = "";
        if (text && text.trim()) {
          setTranscript(text);
          setEditableTranscript(text);
          setHistory((prev) => [{ id: ++historyIdCounter, text, timestamp: new Date().toISOString() }, ...prev].slice(0, 20));
        }
      } else {
        const samples = await invoke("stop_recording") as number[];
        if (samples.length < MIN_TRANSCRIBE_SAMPLES) {
          // Too short to transcribe — the toolbar was already hidden up top.
          setStatus("idle");
          setRecordingTime(0);
          return;
        }

        const text = await invoke("transcribe", { samples, language: languageRef.current }) as string;
        if (text && text.trim()) {
          // Smart cleanup (opt-in): enhance the raw transcript before injecting.
          // Fail-safe — any enhance error falls back to the raw text (never lost).
          let finalText = text;
          if (enhanceEnabledRef.current) {
            setStatus("enhancing");
            try {
              finalText = await invoke("enhance_text", { text, mode: null }) as string;
            } catch (err) {
              console.error("enhance failed, injecting raw transcript:", err);
              finalText = text;
            }
          }
          setTranscript(finalText);
          setEditableTranscript(finalText);
          setHistory((prev) => [{ id: ++historyIdCounter, text: finalText, timestamp: new Date().toISOString() }, ...prev].slice(0, 20));
          // Auto-inject into focused field
          await injectText(finalText);
        }
      }
    } catch (e) {
      setError(String(e));
      // Audible cue so the user notices a failure even when their target app
      // covers the toolbar / main window (bad key, no credit, offline).
      if (audioFeedbackEnabledRef.current) playErrorTone();
    }
    // Toolbar was already hidden at the top of this function (snappy on every path).
    setStatus("idle");
    setRecordingTime(0);
  }, [stopVadPolling, stopTimer, injectText]);


  // Start recording helper — sets always-on-top
  const beginRecording = useCallback(async () => {
    setError("");
    try {
      await invoke("set_vad_enabled", { enabled: vadEnabledRef.current });
      await invoke("set_max_recording_secs", { secs: getMaxRecordingSecs() });
      if (streamingEnabledRef.current) {
        setLivePreview("");
        liveFinalRef.current = "";
        await invoke("start_streaming_transcription", { language: languageRef.current });
      } else {
        await invoke("start_recording");
      }
      // Audio feedback — short ascending arpeggio when mic opens.
      if (audioFeedbackEnabledRef.current) playStartTone();
      // Only swap to the toolbar once the backend accepted the start — avoids
      // leaving the main window hidden behind a toolbar if start_* fails.
      await emit("toolbar-reset").catch(() => {});
      await invoke("show_toolbar_window", { streaming: streamingEnabledRef.current }).catch(() => {});
      setStatus("recording");
      setRecordingTime(0);
      timerRef.current = window.setInterval(() => {
        setRecordingTime((prev) => prev + 0.1);
      }, 100);
      if (!vadPollRef.current) {
        vadPollRef.current = window.setInterval(async () => {
          try {
            const silenceDetected = vadEnabledRef.current ? await invoke("check_silence") as boolean : false;
            const timeoutReached = await invoke("check_timeout") as boolean;
            if ((silenceDetected || timeoutReached) && statusRef.current === "recording") {
              stopAndTranscribe();
            }
          } catch { /* ok */ }
        }, 150);
      }
    } catch (e) {
      setError(String(e));
    }
  }, [stopAndTranscribe]);

  // Hotkey handler
  useEffect(() => {
    const unlistenHotkey = listen<string>("hotkey-pressed", async (event) => {
      const fromToolbar = event.payload === "toolbar";
      // A long batch-view recording owns the mic. The backend already rejects a
      // concurrent start (C1/H1 guards), but its error would land in the hidden
      // main-view error state — bail here so Alt+D is a clean no-op on that screen.
      if (batchRecordingRef.current) return;
      const currentStatus = statusRef.current;
      if (currentStatus === "recording") {
        stopAndTranscribe(fromToolbar);
      } else if (currentStatus === "idle") {
        await beginRecording();
      } else if (currentStatus === "transcribing" && fromToolbar) {
        // Race condition: VAD/timeout auto-stopped recording the same instant
        // the user clicked Stop on the toolbar. Status is already "transcribing"
        // and the toolbar would otherwise stay visible because the listener used
        // to no-op here. Force the toolbar away and surface the main window.
        await invoke("hide_toolbar_window", { forceShowMain: true }).catch(() => {});
      }
    });
    // Separate Pause hotkey — only acts while a recording is active. Toggles
    // pause/resume on the backend, no UI focus change.
    const unlistenPause = listen<string>("pause-pressed", async () => {
      if (statusRef.current !== "recording") return;
      try {
        const isPausedNow = await invoke("is_paused") as boolean;
        if (isPausedNow) {
          await invoke("resume_recording");
        } else {
          await invoke("pause_recording");
        }
      } catch {
        /* state mismatch — ignore, the toolbar's own button is the fallback */
      }
    });
    return () => {
      unlistenHotkey.then((fn) => fn());
      unlistenPause.then((fn) => fn());
    };
  }, [stopAndTranscribe, beginRecording]);

  // Live transcription events (streaming mode). Accumulates final segments and
  // appends the latest interim chunk for in-flight preview.
  useEffect(() => {
    const unlistenInterim = listen<InterimPayload>("transcription-interim", (event) => {
      const { text, is_final } = event.payload;
      if (!text) return;
      if (is_final) {
        liveFinalRef.current = liveFinalRef.current
          ? `${liveFinalRef.current} ${text}`
          : text;
        setLivePreview(liveFinalRef.current);
      } else {
        setLivePreview(
          liveFinalRef.current ? `${liveFinalRef.current} ${text}` : text
        );
      }
    });
    return () => { unlistenInterim.then((fn) => fn()); };
  }, []);

  // Batch transcription progress (decoding / uploading / transcribing / done).
  useEffect(() => {
    const unlisten = listen<{ stage: string; pct: number }>("batch-progress", (event) => {
      setBatchStage(event.payload.stage);
      setBatchPct(Math.round(event.payload.pct ?? 0));
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  const handleToggleRecording = useCallback(async () => {
    const currentStatus = statusRef.current;
    if (currentStatus === "recording") {
      await stopAndTranscribe();
    } else if (currentStatus === "idle") {
      await beginRecording();
    }
  }, [stopAndTranscribe, beginRecording]);

  // Init
  async function refreshModels() {
    try {
      const allModels = await invoke("get_all_models_status") as ModelInfo[];
      setModels(allModels);
      return allModels;
    } catch { return []; }
  }

  // Check for a new release once at startup. Silent on network/endpoint errors.
  useEffect(() => {
    (async () => {
      try {
        const update = await check();
        if (update) {
          updateRef.current = update;
          setUpdateAvailable({ version: update.version });
        }
      } catch {
        // Offline or endpoint unreachable — ignore, try again next launch.
      }
    })();
  }, []);

  const handleInstallUpdate = useCallback(async () => {
    const update = updateRef.current;
    if (!update || updateInstalling) return;
    setUpdateInstalling(true);
    setUpdateProgress(0);
    let contentLength = 0;
    let downloaded = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          contentLength = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength ?? 0;
          if (contentLength > 0) {
            setUpdateProgress(Math.round((downloaded / contentLength) * 100));
          }
        }
      });
      await relaunch();
    } catch (e) {
      setError(`עדכון נכשל: ${e}`);
      setUpdateInstalling(false);
    }
  }, [updateInstalling]);

  useEffect(() => {
    initApp();
    loadDevices();
    const unlistenProgress = listen("model-download-progress", (event) => {
      const data = event.payload as { progress: number };
      setDownloadProgress(data.progress);
    });
    const unlistenClose = listen("window-close-attempted", async () => {
      pendingCloseTipRef.current = true;
    });
    const unlistenFocus = listen("tauri://focus", () => {
      if (pendingCloseTipRef.current) {
        pendingCloseTipRef.current = false;
        setShowCloseTip(true);
      }
    });
    const unlistenMigration = listen<{ status: string; error?: string }>(
      "key-migration",
      (event) => {
        if (event.payload.status === "failed") {
          alert(
            "לא הצלחנו להעביר אוטומטית את מפתחות ה-API לאחסון המאובטח של מערכת ההפעלה. " +
              "המפתח הקיים שלך עדיין שמור בקובץ ההגדרות הישן ופועל. " +
              "מומלץ להזין אותם מחדש בהגדרות כדי שייכנסו לאחסון מאובטח. " +
              "פרטי השגיאה: " +
              (event.payload.error || "לא ידוע")
          );
        }
      }
    );
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenClose.then((fn) => fn());
      unlistenFocus.then((fn) => fn());
      unlistenMigration.then((fn) => fn());
      stopVadPolling();
      stopTimer();
    };
  }, []);

  async function initApp() {
    let preferredModelName = "small";
    let needsOnboarding = true;
    try {
      const settings = await invoke("get_settings") as RedactedSettings;
      setTranscriptionMode(settings.transcription_mode);
      // Default the batch panel toggle from the user's transcription mode.
      setBatchMode(settings.transcription_mode === "local" ? "local" : "cloud");
      setApiProvider(settings.api_provider);
      // Defensive: legacy "auto" language is migrated to "he" by the backend,
      // but if any code path slips through, normalize here too.
      const lang = (settings.language === "auto" ? "he" : settings.language) as Language;
      setLanguage(lang);
      if (settings.language === "auto") {
        persistSettings({ language: "he" });
      }
      setVadEnabled(settings.vad_enabled);
      // Keys are redacted — just track whether they exist on the backend.
      if (settings.has_deepgram_key) setDeepgramKey("••••••••");
      if (settings.has_groq_key) setGroqKey("••••••••");
      setHasGroqKey(!!settings.has_groq_key);
      if (typeof settings.always_on_top === "boolean") setAlwaysOnTop(settings.always_on_top);
      if (typeof settings.autostart_enabled === "boolean") setAutostartEnabled(settings.autostart_enabled);
      if (typeof settings.streaming_enabled === "boolean") {
        const streamingStale = settings.streaming_enabled && settings.api_provider !== "deepgram";
        setStreamingEnabled(streamingStale ? false : settings.streaming_enabled);
        if (streamingStale) {
          invoke("update_settings", { patch: { streaming_enabled: false } }).catch(() => {});
        }
      }
      if (typeof settings.floating_toolbar_enabled === "boolean") setFloatingToolbarEnabled(settings.floating_toolbar_enabled);
      if (typeof settings.hotkey === "string" && settings.hotkey) setHotkey(settings.hotkey);
      if (typeof settings.pause_hotkey === "string" || settings.pause_hotkey === null) {
        setPauseHotkey(settings.pause_hotkey ?? null);
      }
      if (typeof settings.vad_silence_secs === "number") setVadSilenceSecs(settings.vad_silence_secs);
      if (typeof settings.max_recording_secs === "number") setMaxRecordingSecs(settings.max_recording_secs);
      if (typeof settings.unlimited_recording === "boolean") setUnlimitedRecording(settings.unlimited_recording);
      if (typeof settings.preferred_audio_device !== "undefined") {
        setPreferredAudioDevice(settings.preferred_audio_device ?? null);
      }
      if (typeof settings.audio_feedback_enabled === "boolean") {
        setAudioFeedbackEnabled(settings.audio_feedback_enabled);
      }
      if (typeof settings.idle_button_enabled === "boolean") {
        setIdleButtonEnabled(settings.idle_button_enabled);
      }
      if (typeof settings.enhance_enabled === "boolean") {
        setEnhanceEnabled(settings.enhance_enabled);
      }
      if (typeof settings.audio_volume === "number") {
        setAudioVolume(Math.max(0, Math.min(1, settings.audio_volume)));
      }
      if (settings.preferred_model) {
        preferredModelName = settings.preferred_model;
        setSelectedModel(preferredModelName);
      }
      // Skip the wizard if the user already has a working setup — either they've saved
      // an API key (via settings directly) or a local whisper model will be available below.
      // The flag is the source of truth once set; we backfill below for legacy installs.
      const hasKey = settings.has_deepgram_key || settings.has_groq_key;
      needsOnboarding = !settings.onboarding_completed && !hasKey;

      // Backfill the flag so future launches don't recheck (and so it's consistent
      // with the user's saved config even if they never completed the wizard UI).
      if (hasKey && !settings.onboarding_completed) {
        try { await invoke("mark_onboarding_complete"); } catch { /* ok */ }
      }

      // Existing v2.3.x users who already finished onboarding but never accepted the
      // v2.4.0 terms must accept them once before continuing to use the app.
      if (settings.onboarding_completed && !settings.terms_accepted) {
        setShowTermsGate(true);
      }
    } catch { /* defaults */ }

    if (needsOnboarding) setView("onboarding");

    const allModels = await refreshModels();
    const preferred = allModels.find((m) => m.name === preferredModelName && m.downloaded);
    const anyDownloaded = preferred || allModels.find((m) => m.downloaded);
    if (anyDownloaded) {
      setSelectedModel(anyDownloaded.name);
      await loadWhisperModel(anyDownloaded.name, true);
    }
  }

  const persistSettings = useCallback(async (overrides: Partial<AppSettings> = {}) => {
    const settings: AppSettings = {
      transcription_mode: transcriptionMode,
      api_provider: apiProvider,
      preferred_model: selectedModel,
      language: language,
      vad_enabled: vadEnabled,
      always_on_top: alwaysOnTop,
      autostart_enabled: autostartEnabled,
      streaming_enabled: streamingEnabled,
      floating_toolbar_enabled: floatingToolbarEnabled,
      hotkey: hotkey,
      pause_hotkey: pauseHotkey,
      vad_silence_secs: vadSilenceSecs,
      max_recording_secs: maxRecordingSecs,
      unlimited_recording: unlimitedRecording,
      preferred_audio_device: preferredAudioDevice,
      audio_feedback_enabled: audioFeedbackEnabled,
      idle_button_enabled: idleButtonEnabled,
      audio_volume: audioVolume,
      enhance_enabled: enhanceEnabled,
      ...overrides,
    };
    try { await invoke("update_settings", { newSettings: settings }); } catch { /* ok */ }
  }, [transcriptionMode, apiProvider, selectedModel, language, vadEnabled, alwaysOnTop, autostartEnabled, streamingEnabled, floatingToolbarEnabled, hotkey, pauseHotkey, vadSilenceSecs, maxRecordingSecs, unlimitedRecording, preferredAudioDevice, audioFeedbackEnabled, idleButtonEnabled, audioVolume, enhanceEnabled]);

  /** Save an API key to OS-secure storage (Credential Manager / Keychain). */
  const setApiKey = useCallback(async (provider: ApiProvider, key: string) => {
    await invoke("set_api_key", { provider, key });
    // Keep the smart-cleanup gate in sync — it needs a Groq key regardless of
    // which provider transcribes. Without this the toggle stays locked until
    // the next app launch (the load-time setHasGroqKey only runs on startup).
    if (provider === "groq") setHasGroqKey(!!key);
  }, []);

  /** Remove an API key from OS-secure storage. */
  const clearApiKey = useCallback(async (provider: ApiProvider) => {
    await invoke("clear_api_key", { provider });
    if (provider === "groq") setHasGroqKey(false);
  }, []);

  /** Apply a new global hotkey at runtime. Throws on parse error / OS conflict. */
  const applyHotkey = useCallback(async (combo: string) => {
    await invoke("set_hotkey", { combo });
  }, []);

  /** Apply a new Pause hotkey (or `null` to disable). Throws on parse / conflict. */
  const applyPauseHotkey = useCallback(async (combo: string | null) => {
    await invoke("set_pause_hotkey", { combo });
  }, []);

  /**
   * Save the current dictation history to a TXT or DOCX file.
   * Opens an OS save dialog. The saved path is surfaced as a toast.
   */
  const exportHistory = useCallback(async (format: "txt" | "docx") => {
    if (history.length === 0) {
      setError("אין פריטים להיסטוריה — הקלט קודם מספר תמלולים.");
      return;
    }
    setExporting(format);
    setExportNotice(null);
    try {
      const items: ExportHistoryItem[] = history.map((h) => ({ text: h.text, timestamp: h.timestamp }));
      // Name the file after its content (most recent dictation's opening words),
      // same as the batch/file-transcription export — not a generic timestamp.
      const suggested_name = history[0]?.text ? firstWordsName(history[0].text) : "";
      const path = await invoke<string>("export_history", { items, format, suggested_name });
      setExportNotice(`✅ נשמר: ${path}`);
      window.setTimeout(() => setExportNotice(null), 6000);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) {
        setError(`ייצוא ההיסטוריה נכשל: ${msg}`);
      }
    } finally {
      setExporting(null);
    }
  }, [history]);

  // ── Batch file transcription handlers ──
  const handlePickAndTranscribe = useCallback(async () => {
    setBatchError("");
    let filePaths: string[] | null = null;
    try {
      filePaths = await invoke<string[] | null>("pick_audio_files");
    } catch (e) {
      setBatchError(`בחירת הקבצים נכשלה: ${e}`);
      return;
    }
    if (!filePaths || filePaths.length === 0) return;

    const extractFileName = (p: string) =>
      p.replace(/\\/g, "/").split("/").pop() || p;

    const initial: BatchResult[] = filePaths.map((p) => ({
      id: ++batchIdCounter,
      fileName: extractFileName(p),
      filePath: p,
      transcript: "",
      status: "pending",
    }));

    // Append the new files, preserving any existing results (button = "הוסף קבצים").
    // All per-item updates below target by ID, not index, so appended items are
    // matched correctly regardless of how many results already existed.
    setBatchResults((prev) => [...prev, ...initial]);
    setBatchRunning(true);
    setBatchFileTotal(initial.length);
    setBatchCurrentIdx(0);
    batchCancelledRef.current = false;

    for (let i = 0; i < initial.length; i++) {
      if (batchCancelledRef.current) {
        const remaining = new Set(initial.slice(i).map((r) => r.id));
        setBatchResults((prev) =>
          prev.map((r) => remaining.has(r.id) ? { ...r, status: "cancelled" } : r)
        );
        break;
      }

      const curId = initial[i].id;
      setBatchCurrentIdx(i);
      setBatchActiveResultId(curId);
      setBatchPct(0);
      setBatchStage("decoding");
      setBatchResults((prev) =>
        prev.map((r) => r.id === curId ? { ...r, status: "processing" } : r)
      );

      try {
        const { text, segments } = await invoke<{ text: string; segments: TimedSegment[] }>(
          "transcribe_file",
          { filePath: initial[i].filePath, opts: { mode: batchMode, language: "he", inject: false } }
        );
        setBatchResults((prev) =>
          prev.map((r) => r.id === curId ? { ...r, status: "done", transcript: text, segments } : r)
        );
      } catch (e) {
        const msg = String(e);
        if (msg === "בוטל" || batchCancelledRef.current) {
          const remaining = new Set(initial.slice(i).map((r) => r.id));
          setBatchResults((prev) =>
            prev.map((r) => remaining.has(r.id) ? { ...r, status: "cancelled" } : r)
          );
          break;
        }
        setBatchResults((prev) =>
          prev.map((r) => r.id === curId ? { ...r, status: "error", error: msg } : r)
        );
      }
    }

    setBatchRunning(false);
    setBatchStage("done");
    setBatchActiveResultId(null);
  }, [batchMode]);

  const handleCancelBatch = useCallback(async () => {
    batchCancelledRef.current = true;
    try {
      await invoke("cancel_batch");
    } catch {
      /* ignore */
    }
  }, []);

  const formatRecordTime = (secs: number) => {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    return h > 0
      ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
      : `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  };

  const handleStartBatchRecord = useCallback(async () => {
    setBatchError("");
    try {
      // Thread the chosen source (Mic/System/Call) into the record command.
      // Default "mic" reproduces the previous no-arg behavior exactly.
      await invoke("start_batch_recording", { source: recordSource });
      setBatchRecording(true);
      setBatchRecordElapsed(0);
      batchRecordTimerRef.current = setInterval(() => {
        setBatchRecordElapsed((e) => e + 1);
      }, 1000);
    } catch (e) {
      setBatchError(String(e));
    }
  }, [recordSource]);

  const handleStopBatchRecord = useCallback(async () => {
    if (batchRecordTimerRef.current) {
      clearInterval(batchRecordTimerRef.current);
      batchRecordTimerRef.current = null;
    }
    setBatchRecording(false);

    // Call is backend-only (spec §4.3/§4.6): it never writes a WAV and must NOT go
    // through transcribe_file → decode_file_to_16k_mono, which merges to mono and
    // destroys the channel separation ("הצד השני"). It stops both recorders and
    // returns (text, segments) directly. Mic/System keep the existing file path,
    // now passing `source` so the backend drains the right recorder (System routes
    // to system_recorder inside stop_batch_recording_to_file).
    const isCallCloud = recordSource === "callcloud";

    let filePath = "";
    if (!isCallCloud) {
      try {
        filePath = await invoke<string>("stop_batch_recording_to_file", { source: recordSource });
      } catch (e) {
        setBatchError(`שמירת ההקלטה נכשלה: ${String(e)}`);
        return;
      }
    }

    const now = new Date();
    const timeStr = now.toLocaleTimeString("he-IL", { hour: "2-digit", minute: "2-digit" });
    const fileName = `הקלטה ${timeStr}.wav`;
    const newId = ++batchIdCounter;
    const newItem: BatchResult = { id: newId, fileName, filePath, transcript: "", status: "pending", isCallCloud };

    setBatchActiveResultId(newId);
    setBatchResults((prev) => [...prev, newItem]);
    setBatchRunning(true);
    setBatchPct(0);
    setBatchStage(isCallCloud ? "transcribing" : "decoding");
    batchCancelledRef.current = false;

    setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "processing" } : r));

    try {
      // calllocal is FORCED local (its card encodes the engine, so the cloud/local
      // selector is hidden for it); mic/system honor the batchMode selector.
      const fileMode = recordSource === "calllocal" ? "local" : batchMode;
      // CallCloud → dedicated backend command (stops both recorders, interleaves,
      // multichannel Deepgram, returns tagged "אני:"/"הצד השני:" text + merged
      // segments). Mic/System/CallLocal → existing mono file path.
      const { text, segments } = isCallCloud
        ? await invoke<{ text: string; segments: TimedSegment[] }>("stop_call_recording", {
            opts: { mode: batchMode, language: "he", inject: false },
          })
        : await invoke<{ text: string; segments: TimedSegment[] }>(
            "transcribe_file",
            { filePath, opts: { mode: fileMode, language: "he", inject: false } }
          );
      setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "done", transcript: text, segments } : r));
    } catch (e) {
      const msg = String(e);
      if (msg === "בוטל" || batchCancelledRef.current) {
        setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "cancelled" } : r));
      } else {
        setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "error", error: msg } : r));
      }
    } finally {
      // Only Mic/System write a temp WAV (≈110 MB/hour); Call is in-memory so there
      // is no file to delete. Guard on filePath so the Call branch skips cleanup.
      if (filePath) {
        try { await invoke("delete_temp_recording", { path: filePath }); } catch { /* ignore */ }
      }
    }

    setBatchRunning(false);
    setBatchStage("done");
    setBatchActiveResultId(null);
  }, [batchMode, recordSource]);

  const handleCancelBatchRecord = useCallback(async () => {
    if (batchRecordTimerRef.current) {
      clearInterval(batchRecordTimerRef.current);
      batchRecordTimerRef.current = null;
    }
    setBatchRecording(false);
    setBatchRecordElapsed(0);
    try { await invoke("cancel_batch_recording"); } catch { /* ignore */ }
  }, []);

  const generateExportName = (results: BatchResult[]): string => {
    const first = results.find((r) => r.status === "done" && r.transcript.trim());
    return first ? firstWordsName(first.transcript) : "";
  };

  /** True when a batch item has usable timed segments for SRT export (done, not
   * hand-edited since transcription, and has at least one segment). Single source
   * of truth — used by the per-item button, the combined button, and exportBatchSrt. */
  const isSrtEligible = (r: BatchResult): boolean =>
    r.status === "done" && !r.edited && !!r.segments && r.segments.length > 0;

  // Per-item export: save a single transcript segment to TXT/DOCX, named by content.
  const exportSingle = useCallback(async (
    text: string,
    format: "txt" | "docx",
    onErr: (msg: string) => void,
  ) => {
    const t = text.trim();
    if (!t) return;
    try {
      await invoke<string>("export_history", {
        items: [{ text: t, timestamp: new Date().toISOString() }],
        format,
        suggested_name: firstWordsName(t),
      });
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) onErr(`ייצוא נכשל: ${msg}`);
    }
  }, []);

  const exportBatch = useCallback(async (format: "txt" | "docx") => {
    const done = batchResults.filter((r) => r.status === "done" && r.transcript.trim());
    if (done.length === 0) return;
    try {
      const items = done.map((r) => ({ text: r.transcript, timestamp: new Date().toISOString() }));
      const suggested_name = generateExportName(done);
      const path = await invoke<string>("export_history", { items, format, suggested_name });
      setExportNotice(`✅ נשמר: ${path}`);
      window.setTimeout(() => setExportNotice(null), 6000);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) setBatchError(`ייצוא נכשל: ${msg}`);
    }
  }, [batchResults]);

  const exportSingleSrt = useCallback(async (
    segments: TimedSegment[],
    transcriptForName: string,
    isCallCloud: boolean | undefined,
    onErr: (msg: string) => void,
  ) => {
    if (segments.length === 0) return;
    try {
      await invoke<string>("export_srt", {
        items: [segments],
        styles: [isCallCloud ? "Call" : "Diarization"],
        suggested_name: firstWordsName(transcriptForName),
      });
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) onErr(`ייצוא נכשל: ${msg}`);
    }
  }, []);

  const exportBatchSrt = useCallback(async () => {
    const eligible = batchResults.filter(isSrtEligible);
    if (eligible.length === 0) return;
    try {
      const items = eligible.map((r) => r.segments!);
      const styles = eligible.map((r) => (r.isCallCloud ? "Call" : "Diarization"));
      const suggested_name = generateExportName(eligible);
      const path = await invoke<string>("export_srt", { items, styles, suggested_name });
      setExportNotice(`✅ נשמר: ${path}`);
      window.setTimeout(() => setExportNotice(null), 6000);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) setBatchError(`ייצוא נכשל: ${msg}`);
    }
  }, [batchResults]);

  /** Push the silence-to-stop duration to the running recorder. */
  const applySilenceDuration = useCallback(async (secs: number) => {
    await invoke("set_silence_duration_secs", { secs }).catch(() => {});
  }, []);

  /** Push the max recording duration to the running recorder. */
  const applyMaxRecording = useCallback(async (secs: number) => {
    await invoke("set_max_recording_secs", { secs }).catch(() => {});
  }, []);

  /** Push the preferred audio device to the running recorder. */
  const applyPreferredDevice = useCallback(async (device: string | null) => {
    await invoke("set_preferred_audio_device", { device }).catch(() => {});
  }, []);

  async function handleTestApiKey() {
    const activeKey = apiProvider === "groq" ? groqKey : deepgramKey;
    if (!activeKey) return;
    setTestingApiKey(true);
    setApiKeyValid(null);
    try {
      await invoke("test_api_key", { provider: apiProvider, apiKey: activeKey });
      setApiKeyValid(true);
      // Persist the key (the input onBlur may not have fired) and reflect it for
      // the cleanup toggle — a successful test means we have a usable key.
      await setApiKey(apiProvider, activeKey);
    } catch { setApiKeyValid(false); }
    setTestingApiKey(false);
  }

  // Test the dedicated cleanup Groq key (independent of the selected transcription
  // provider). On success, persist it — which unlocks the cleanup toggle.
  async function handleTestGroqCleanup() {
    if (!groqKey || groqKey === "••••••••") return;
    setTestingGroqCleanup(true);
    setGroqCleanupValid(null);
    try {
      await invoke("test_api_key", { provider: "groq", apiKey: groqKey });
      setGroqCleanupValid(true);
      await setApiKey("groq", groqKey);
    } catch { setGroqCleanupValid(false); }
    setTestingGroqCleanup(false);
  }

  async function loadDevices() {
    try {
      const devs = await invoke("get_audio_devices");
      setDevices(devs as string[]);
    } catch (e) { setError(String(e)); }
  }

  async function loadWhisperModel(modelName?: string, background = false) {
    const name = modelName || selectedModel;
    // Foreground (user-initiated model switch) blocks with a status; the background
    // (startup) load stays OUT of the status machine so recording / Alt+D / the
    // floating bar work immediately, even while a large model is still loading.
    if (background) setModelLoading(true); else setStatus("loading-model");
    try {
      await invoke("load_whisper_model", { modelName: name });
      setWhisperLoaded(true);
      setActiveModel(name);
      setSelectedModel(name);
      if (!background) setStatus("idle");
    } catch (e) {
      setError(String(e));
      if (!background) setStatus("idle");
    } finally {
      if (background) setModelLoading(false);
    }
  }

  async function handleDownloadModel(modelName: string) {
    setStatus("downloading");
    setDownloadingModel(modelName);
    setError("");
    setDownloadProgress(0);
    try {
      await invoke("download_model", { modelName });
      await refreshModels();
      setStatus("idle");
      setDownloadingModel(null);
      if (!activeModel) await loadWhisperModel(modelName);
    } catch (e) { setError(String(e)); setStatus("idle"); setDownloadingModel(null); }
  }

  async function handleDeleteModel(modelName: string) {
    setError("");
    try {
      await invoke("delete_model", { modelName });
      if (activeModel === modelName) { setActiveModel(null); setWhisperLoaded(false); }
      await refreshModels();
    } catch (e) { setError(String(e)); }
  }

  // Computed
  const downloadedCount = models.filter((m) => m.downloaded).length;
  // User override takes precedence; unlimited disables the countdown entirely.
  const effectiveMaxRecordingSecs = unlimitedRecording
    ? Infinity
    : (maxRecordingSecs > 0
        ? maxRecordingSecs
        : (transcriptionMode === "local" ? MAX_RECORDING_LOCAL : MAX_RECORDING_API));
  const timeRemaining = unlimitedRecording ? Infinity : (effectiveMaxRecordingSecs - recordingTime);
  const showTimeWarning = status === "recording" && Number.isFinite(timeRemaining) && timeRemaining <= 10;
  const activeApiKey = apiProvider === "groq" ? groqKey : deepgramKey;
  const apiKeyConfigured = transcriptionMode !== "local" && activeApiKey.length > 0;
  const canRecord = whisperLoaded || apiKeyConfigured;
  const langLabels: Record<Language, string> = { he: "עברית", en: "English", multi: "עברית + אנגלית" };
  const modeLabel = transcriptionMode === "api" ? "API" : transcriptionMode === "local" ? "מקומי" : "אוטומטי";

  // ---- ONBOARDING WIZARD ----
  if (view === "onboarding") {
    const handleWizardTestKey = async () => {
      if (!wizardApiKey) return;
      setWizardKeyTesting(true);
      setWizardKeyValid(null);
      try {
        await invoke("test_api_key", { provider: wizardProviderKey as ApiProvider, apiKey: wizardApiKey });
        setWizardKeyValid(true);
      } catch { setWizardKeyValid(false); }
      setWizardKeyTesting(false);
    };

    const completeOnboarding = async () => {
      // Persist key first, but never let a keyring failure trap the user inside
      // the wizard on every launch. v2.8.x bug: setApiKey would throw on a
      // locked-down Credential Manager / antivirus block, the wizard would
      // abort before persistSettings, onboarding_completed stayed false, and
      // the wizard re-ran every launch. Now: capture the error, finish the
      // wizard, and surface a toast on the main view.
      let keyError: string | null = null;
      try {
        if (wizardChoice === "api" && wizardApiKey) {
          await setApiKey("deepgram", wizardApiKey);
          setDeepgramKey("••••••••");
        } else if (wizardChoice === "groq" && wizardApiKey) {
          await setApiKey("groq", wizardApiKey);
          setGroqKey("••••••••");
        }
      } catch (e) {
        keyError = String(e);
      }

      // Apply chosen mode regardless of key save outcome.
      const overrides: Partial<AppSettings> = { onboarding_completed: true };
      if (wizardChoice === "api") {
        setApiProvider("deepgram");
        setTranscriptionMode("api");
        overrides.api_provider = "deepgram";
        overrides.transcription_mode = "api";
      } else if (wizardChoice === "groq") {
        setApiProvider("groq");
        setTranscriptionMode("api");
        setStreamingEnabled(false);
        overrides.api_provider = "groq";
        overrides.transcription_mode = "api";
        overrides.streaming_enabled = false;
      } else if (wizardChoice === "local") {
        setTranscriptionMode("local");
        overrides.transcription_mode = "local";
      }

      // ALWAYS persist onboarding_completed=true — even if setApiKey above
      // failed. The user can re-enter the key from Settings; we don't want them
      // stuck in the wizard forever.
      try { await persistSettings(overrides); } catch { /* swallow — best effort */ }
      try { await invoke("accept_terms"); } catch { /* ok */ }
      setView("main");

      if (keyError) {
        setError(
          `המפתח לא נשמר באחסון המאובטח (Credential Manager). נסה שוב מההגדרות. פרטים: ${keyError}`
        );
      }
    };

    const termsAccepted = wizardTermsAsIs && wizardTermsKeys;

    return (
      <main className="container compact" dir="rtl">
        <div className="wizard-dots">
          {[1, 2, 3, 4].map((s) => (
            <span key={s} className={`wizard-dot ${wizardStep === s ? "active" : wizardStep > s ? "done" : ""}`} />
          ))}
        </div>

        {wizardStep === 1 && (
          <div className="wizard-step">
            <h1 className="wizard-title">🎤 הכתבה בעברית</h1>
            <p className="wizard-subtitle">by BinTech AI — קוד פתוח</p>
            <div className="wizard-content">
              <p>הכתבה קולית בעברית מכל מקום במחשב.</p>
              <div className="wizard-highlight">
                <span className="wizard-key">Alt + D</span>
                <span>להקלטה ועצירה</span>
              </div>
              <p className="wizard-note">הטקסט מוקלד אוטומטית בשדה שבו העכבר נמצא.</p>
            </div>
            <button className="btn-wizard-next" onClick={() => setWizardStep(2)}>המשך</button>
          </div>
        )}

        {wizardStep === 2 && (
          <div className="wizard-step">
            <h2 className="wizard-step-title">לפני שמתחילים — הצהרת שימוש</h2>
            <p className="wizard-subtitle" style={{ marginBottom: "0.8rem" }}>
              התוכנה חינמית וקוד פתוח. כמה דברים שחשוב להבין לפני התחלה.
            </p>
            <div className="wizard-content" style={{ display: "flex", flexDirection: "column", gap: "0.5rem", textAlign: "right" }}>
              <div className="wizard-card" style={{ cursor: "default", padding: "0.6rem 0.8rem" }}>
                <div className="wizard-card-header">
                  <span style={{ fontWeight: 600 }}>🛡 התוכנה ניתנת ״כפי שהיא״</span>
                </div>
                <p className="wizard-card-desc" style={{ fontSize: "0.78rem" }}>
                  אין אחריות לדיוק התמלול. אסור להסתמך עליו לבדו במצבים קריטיים — רפואי, משפטי, פיננסי.
                </p>
              </div>
              <div className="wizard-card" style={{ cursor: "default", padding: "0.6rem 0.8rem" }}>
                <div className="wizard-card-header">
                  <span style={{ fontWeight: 600 }}>💳 רק מפתחות בלי כרטיס אשראי</span>
                </div>
                <p className="wizard-card-desc" style={{ fontSize: "0.78rem" }}>
                  תומכים רק ב-Deepgram ו-Groq — שניהם נותנים מפתח חינם בלי להזין אשראי. אם תבחרו לטעון קרדיט בתשלום — זה ביניכם לבין הספק.
                </p>
              </div>
              <div className="wizard-card" style={{ cursor: "default", padding: "0.6rem 0.8rem" }}>
                <div className="wizard-card-header">
                  <span style={{ fontWeight: 600 }}>🔑 אחריות על המפתח עליכם</span>
                </div>
                <p className="wizard-card-desc" style={{ fontSize: "0.78rem" }}>
                  המפתח שלכם נשמר רק על המחשב שלכם. לא אצלנו. שמירתו בסוד ורענון אם דלף — באחריותכם.
                </p>
              </div>
              <div className="wizard-card" style={{ cursor: "default", padding: "0.6rem 0.8rem" }}>
                <div className="wizard-card-header">
                  <span style={{ fontWeight: 600 }}>⚖ אסור להקליט אדם בלי הסכמתו</span>
                </div>
                <p className="wizard-card-desc" style={{ fontSize: "0.78rem" }}>
                  חוק האזנת סתר תשל״ט-1979. השימוש בתוכנה לצורך זה אסור והאחריות החוקית עליכם.
                </p>
              </div>

              <a
                href={TERMS_FULL_URL}
                target="_blank"
                rel="noopener"
                className="link-text"
                style={{ fontSize: "0.78rem", marginTop: "0.2rem" }}
              >
                לתנאי השימוש המלאים באתר →
              </a>

              <label className="toggle-label" style={{ marginTop: "0.4rem", alignItems: "flex-start", gap: "0.5rem" }}>
                <input
                  type="checkbox"
                  checked={wizardTermsAsIs}
                  onChange={() => setWizardTermsAsIs(!wizardTermsAsIs)}
                />
                <span className="toggle-text" style={{ fontSize: "0.82rem" }}>
                  קראתי והבנתי שהתוכנה ניתנת ״כפי שהיא״ ללא אחריות לדיוק התמלול
                </span>
              </label>
              <label className="toggle-label" style={{ alignItems: "flex-start", gap: "0.5rem" }}>
                <input
                  type="checkbox"
                  checked={wizardTermsKeys}
                  onChange={() => setWizardTermsKeys(!wizardTermsKeys)}
                />
                <span className="toggle-text" style={{ fontSize: "0.82rem" }}>
                  אני מאשר שאני אחראי על מפתחות ה-API שלי ועל השימוש בהם
                </span>
              </label>
            </div>
            <div className="wizard-nav">
              <button className="btn-wizard-back" onClick={() => setWizardStep(1)}>חזור</button>
              <button
                className="btn-wizard-next"
                onClick={() => setWizardStep(3)}
                disabled={!termsAccepted}
                title={!termsAccepted ? "סמנו את שני התנאים כדי להמשיך" : ""}
              >
                {termsAccepted ? "מסכים, המשך" : "סמנו את שני התנאים"}
              </button>
            </div>
          </div>
        )}

        {wizardStep === 3 && (
          <div className="wizard-step">
            <h2 className="wizard-step-title">בחר מצב תמלול</h2>

            <div
              className={`wizard-card ${wizardChoice === "api" ? "selected" : ""}`}
              onClick={() => {
                setWizardChoice("api");
                setWizardProviderKey("deepgram");
                setWizardApiKey("");
                setWizardKeyValid(null);
              }}
            >
              <div className="wizard-card-header">
                <strong>☁️ Deepgram — מהיר ומדויק</strong>
                <span className="wizard-card-badge">מומלץ</span>
              </div>
              <p className="wizard-card-desc">תמלול בענן דרך Deepgram Nova-3. הדיוק הגבוה ביותר בעברית.</p>
              <ul className="wizard-card-facts">
                <li>✅ <strong>~50 שעות חינם</strong> עם $200 קרדיט של Deepgram</li>
                <li>✅ <strong>ללא כרטיס אשראי</strong> — אי אפשר לחייב אותך בטעות</li>
                <li>✅ מהירות: 1-2 שניות מסוף הדיבור לטקסט</li>
                <li>⚠ דורש אינטרנט + האודיו נשלח ל-Deepgram</li>
                <li>💡 כשהקרדיט נגמר — אפשר לעבור ל-Groq (זול פי 5) או למצב מקומי</li>
              </ul>
              {wizardChoice === "api" && (
                <div className="wizard-guide">
                  <p className="wizard-guide-title">📋 איך מוציאים מפתח (חינם):</p>
                  <ol>
                    <li>
                      לחץ כאן →{" "}
                      <a href="https://console.deepgram.com/signup" target="_blank" rel="noopener" className="link-text">
                        deepgram.com — הרשמה
                      </a>
                    </li>
                    <li>צור חשבון חינם (אימייל או Google) — <strong>בלי כרטיס אשראי</strong></li>
                    <li>לחץ <strong>Create API Key</strong> בדף הראשי</li>
                    <li>העתק את המפתח והדבק כאן:</li>
                  </ol>
                  <div className="api-key-row">
                    <input
                      type="password"
                      className="api-key-input"
                      value={wizardApiKey}
                      onChange={(e) => { setWizardApiKey(e.target.value); setWizardKeyValid(null); }}
                      placeholder="הדבק מפתח Deepgram..."
                    />
                    <button
                      className={`btn-test ${wizardKeyValid === true ? "valid" : wizardKeyValid === false ? "invalid" : ""}`}
                      onClick={handleWizardTestKey}
                      disabled={wizardKeyTesting || !wizardApiKey}
                    >
                      {wizardKeyTesting ? "..." : wizardKeyValid === true ? "✓" : wizardKeyValid === false ? "✗" : "בדוק"}
                    </button>
                  </div>
                  {wizardKeyValid === true && <p className="settings-note success-note">✅ המפתח תקין!</p>}
                  {wizardKeyValid === false && <p className="settings-note error-note">❌ המפתח לא תקין</p>}
                  <p className="wizard-note" style={{ fontSize: "0.7rem", marginTop: "0.3rem" }}>
                    💡 המפתח נשמר רק אצלך במחשב. לא נשלח לשום מקום חוץ מ-Deepgram.
                  </p>
                </div>
              )}
            </div>

            <div
              className={`wizard-card ${wizardChoice === "local" ? "selected" : ""}`}
              onClick={() => setWizardChoice("local")}
            >
              <div className="wizard-card-header">
                <strong>💻 מקומי — פרטיות מלאה</strong>
                <span className="wizard-card-badge">חינם לעד</span>
              </div>
              <p className="wizard-card-desc">תמלול Whisper שרץ על המחשב שלך. האודיו לא יוצא החוצה.</p>
              <ul className="wizard-card-facts">
                <li>✅ <strong>חינם לעד</strong> — ללא חשבון, ללא קרדיט, ללא הגבלה</li>
                <li>✅ פרטיות מלאה — האודיו לא עוזב את המחשב</li>
                <li>✅ עובד אופליין — בלי חיבור לאינטרנט</li>
                <li>⚠ איטי יותר: 5-10 שניות עיבוד (תלוי בחוזק המחשב)</li>
                <li>⚠ דורש הורדת מודל חד-פעמית: 75MB עד 1.5GB</li>
                <li>⚠ דיוק בעברית סביר, אבל נמוך מ-Deepgram</li>
              </ul>
            </div>

            <div
              className={`wizard-card ${wizardChoice === "groq" ? "selected" : ""}`}
              onClick={() => {
                setWizardChoice("groq");
                setWizardProviderKey("groq");
                setWizardApiKey("");
                setWizardKeyValid(null);
              }}
            >
              <div className="wizard-card-header">
                <strong>⚡ Groq — חלופה אחרי שה-$200 נגמרים</strong>
                <span className="wizard-card-badge">חלופה</span>
              </div>
              <p className="wizard-card-desc">Whisper Turbo דרך Groq. ~$0.04/שעה — פי 5 יותר זול מ-Deepgram (אבל פחות מדויק).</p>
              <ul className="wizard-card-facts">
                <li>✅ <strong>~$0.04/שעה</strong> — הזול בשוק (Free tier מוגבל)</li>
                <li>✅ מהירות: 1-2 שניות</li>
                <li>⚠ דיוק בעברית מעט נמוך מ-Deepgram</li>
                <li>⚠ <strong>ללא תמלול סימולטני</strong> (תמלול רק אחרי שלוחצים ״עצור״)</li>
                <li>💡 רלוונטי בעיקר אחרי שה-$200 של Deepgram ניצלו</li>
              </ul>
              {wizardChoice === "groq" && (
                <div className="wizard-guide">
                  <p className="wizard-guide-title">📋 איך מוציאים מפתח (חינם):</p>
                  <ol>
                    <li>
                      לחץ כאן →{" "}
                      <a href="https://console.groq.com/keys" target="_blank" rel="noopener" className="link-text">
                        console.groq.com/keys
                      </a>
                    </li>
                    <li>התחבר עם Google / GitHub / Email</li>
                    <li>לחץ <strong>Create API Key</strong></li>
                    <li>העתק את המפתח (מתחיל ב-<code>gsk_</code>) והדבק כאן:</li>
                  </ol>
                  <div className="api-key-row">
                    <input
                      type="password"
                      className="api-key-input"
                      value={wizardApiKey}
                      onChange={(e) => { setWizardApiKey(e.target.value); setWizardKeyValid(null); }}
                      placeholder="gsk_..."
                    />
                    <button
                      className={`btn-test ${wizardKeyValid === true ? "valid" : wizardKeyValid === false ? "invalid" : ""}`}
                      onClick={handleWizardTestKey}
                      disabled={wizardKeyTesting || !wizardApiKey}
                    >
                      {wizardKeyTesting ? "..." : wizardKeyValid === true ? "✓" : wizardKeyValid === false ? "✗" : "בדוק"}
                    </button>
                  </div>
                  {wizardKeyValid === true && <p className="settings-note success-note">✅ המפתח תקין!</p>}
                  {wizardKeyValid === false && <p className="settings-note error-note">❌ המפתח לא תקין</p>}
                  <p className="wizard-note" style={{ fontSize: "0.7rem", marginTop: "0.3rem" }}>
                    💡 המפתח נשמר רק אצלך במחשב. לא נשלח לשום מקום חוץ מ-Groq.
                  </p>
                </div>
              )}
            </div>

            <div className="wizard-nav">
              <button className="btn-wizard-back" onClick={() => setWizardStep(2)}>חזור</button>
              <button className="btn-wizard-next" onClick={() => setWizardStep(4)} disabled={!wizardChoice}>
                {wizardChoice ? "המשך" : "בחר מצב"}
              </button>
            </div>
          </div>
        )}

        {wizardStep === 4 && (
          <div className="wizard-step">
            <h2 className="wizard-step-title">✅ הכל מוכן!</h2>
            <div className="wizard-content">
              {wizardChoice === "api" && wizardApiKey ? (
                <p className="wizard-success">Deepgram מוגדר — תמלול מהיר ומדויק</p>
              ) : wizardChoice === "groq" && wizardApiKey ? (
                <p className="wizard-success">Groq מוגדר — תמלול מהיר וזול</p>
              ) : wizardChoice === "local" ? (
                <p className="wizard-note">מצב מקומי — הורד מודל בהגדרות כדי להתחיל</p>
              ) : (
                <p className="wizard-note">הגדר מפתח API או הורד מודל בהגדרות</p>
              )}
              <div className="wizard-highlight">
                <span>לחץ</span>
                <span className="wizard-key">Alt + D</span>
                <span>ודבר בעברית</span>
              </div>
              <p className="wizard-note" style={{ fontSize: "0.7rem" }}>התוכנה רצה ברקע. גם בסגירת החלון Alt+D ממשיך לעבוד.</p>

              <label className="toggle-label wizard-idle-toggle">
                <input
                  type="checkbox"
                  checked={idleButtonEnabled}
                  onChange={() => {
                    const v = !idleButtonEnabled;
                    setIdleButtonEnabled(v);
                    invoke("set_idle_button_enabled", { enabled: v }).catch(() => {});
                    persistSettings({ idle_button_enabled: v });
                  }}
                />
                <span className="toggle-text">כפתור צף תמידי — לחיצה אחת להכתבה, בלי לזכור קיצור</span>
              </label>

              <div className="wizard-cta-block">
                <p className="wizard-cta-title">אהבתם? עקבו לעוד כלי AI מעולים בעברית</p>
                <div className="wizard-cta-grid">
                  <a className="wizard-cta-btn cta-youtube" href={LINKS.youtube} target="_blank" rel="noopener">
                    <span className="cta-icon">🎥</span>
                    <span className="cta-label">YouTube</span>
                  </a>
                  <a className="wizard-cta-btn cta-whatsapp" href={LINKS.whatsappChannel} target="_blank" rel="noopener">
                    <span className="cta-icon">💬</span>
                    <span className="cta-label">WhatsApp</span>
                  </a>
                  <a className="wizard-cta-btn cta-taplink" href={LINKS.taplink} target="_blank" rel="noopener">
                    <span className="cta-icon">🔗</span>
                    <span className="cta-label">כל הקישורים</span>
                  </a>
                  <a className="wizard-cta-btn cta-feedback" href={FEEDBACK_URL} target="_blank" rel="noopener">
                    <span className="cta-icon">✏️</span>
                    <span className="cta-label">פידבק</span>
                  </a>
                </div>
              </div>
            </div>
            <div className="wizard-final-actions">
              <button className="btn-wizard-next" onClick={completeOnboarding}>התחל</button>
            </div>
            <div className="wizard-credit">
              <span>נוצר ע״י הנרי שטאובר · BinTech AI · רישיון {APP_LICENSE} · {APP_VERSION}</span>
            </div>
          </div>
        )}
      </main>
    );
  }

  // ---- SETTINGS VIEW ----
  if (view === "settings") {
    return (
      <main className="container compact" dir="rtl">
        <div className="settings-header">
          <h2>הגדרות</h2>
          <button className="btn-back" onClick={() => setView(settingsReturn)}>חזור</button>
        </div>

        {/* Engine */}
        <div className="settings-section">
          <h3>מנוע תמלול</h3>
          <div className="settings-row">
            {([
              ["api", "API (ענן)", "מהיר ומדויק, עולה קרדיט"],
              ["local", "מקומי", "אופליין, איטי יותר"],
              ["auto_fallback", "אוטומטי", "API אם יש, אחרת מקומי"],
            ] as [TranscriptionMode, string, string][]).map(([mode, label, sub]) => (
              <button
                key={mode}
                className={`btn-option btn-option-stack ${transcriptionMode === mode ? "active" : ""}`}
                onClick={() => { setTranscriptionMode(mode); persistSettings({ transcription_mode: mode }); }}
                title={sub}
              >
                <span className="btn-option-label">{label}</span>
                <span className="btn-option-sub">{sub}</span>
              </button>
            ))}
          </div>
        </div>

        {/* API Key */}
        {transcriptionMode !== "local" && (
          <div className="settings-section">
            <h3>ספק API</h3>
            <div className="settings-row" style={{ marginBottom: "0.5rem" }}>
              {([
                ["deepgram", "Deepgram"],
                ["groq", "Groq"],
              ] as [ApiProvider, string][]).map(([prov, label]) => (
                <button
                  key={prov}
                  className={`btn-option ${apiProvider === prov ? "active" : ""}`}
                  onClick={() => {
                    setApiProvider(prov);
                    setApiKeyValid(null);
                    const patch: Record<string, unknown> = { api_provider: prov };
                    if (prov !== "deepgram" && streamingEnabled) {
                      setStreamingEnabled(false);
                      patch.streaming_enabled = false;
                    }
                    persistSettings(patch);
                  }}
                >
                  {label}
                </button>
              ))}
            </div>
            <p className="settings-note provider-fact">
              {apiProvider === "deepgram"
                ? "💰 $200 קרדיט חינם ≈ 50 שעות הכתבה. ללא כרטיס אשראי. דיוק הכי גבוה בעברית."
                : "💰 Groq Whisper Turbo: ~$0.04/שעה — הכי זול. יש Free tier מוגבל. ללא streaming."}
            </p>
            <div className="api-key-row">
              <input
                type="password"
                className="api-key-input"
                value={apiProvider === "groq" ? groqKey : deepgramKey}
                onChange={(e) => {
                  if (apiProvider === "groq") setGroqKey(e.target.value);
                  else setDeepgramKey(e.target.value);
                  setApiKeyValid(null);
                }}
                onBlur={async () => {
                  const provider = apiProvider;
                  const value = provider === "groq" ? groqKey : deepgramKey;
                  // Skip the masked placeholder — that means the existing key wasn't touched.
                  if (value === "••••••••") return;
                  try {
                    if (value) {
                      await setApiKey(provider, value);
                    } else {
                      await clearApiKey(provider);
                    }
                  } catch { /* swallow — UI keeps its local state either way */ }
                }}
                placeholder={apiProvider === "groq" ? "gsk_..." : "API key..."}
              />
              <button
                className={`btn-test ${apiKeyValid === true ? "valid" : apiKeyValid === false ? "invalid" : ""}`}
                onClick={handleTestApiKey}
                disabled={testingApiKey || !activeApiKey}
              >
                {testingApiKey ? "..." : apiKeyValid === true ? "✓" : apiKeyValid === false ? "✗" : "בדוק"}
              </button>
            </div>
            {apiKeyValid === false && <p className="settings-note error-note">המפתח לא תקין</p>}
            {apiKeyValid === true && <p className="settings-note success-note">המפתח תקין</p>}
            <div className="settings-links-row">
              <p className="settings-note">
                {apiProvider === "deepgram" ? (
                  <a href="https://console.deepgram.com/signup" target="_blank" rel="noopener" className="link-text">קבל מפתח חינם → deepgram.com</a>
                ) : (
                  <a href="https://console.groq.com/keys" target="_blank" rel="noopener" className="link-text">קבל מפתח חינם → groq.com</a>
                )}
              </p>
              <p className="settings-note">
                {apiProvider === "deepgram" ? (
                  <a href="https://console.deepgram.com/project/default/usage" target="_blank" rel="noopener" className="link-text">כמה קרדיט נשאר? →</a>
                ) : (
                  <a href="https://console.groq.com/settings/usage" target="_blank" rel="noopener" className="link-text">בדוק שימוש →</a>
                )}
              </p>
            </div>
          </div>
        )}

        {/* Language */}
        <div className="settings-section">
          <h3>שפה</h3>
          <div className="settings-row">
            {(["he", "en", "multi"] as Language[]).map((lang) => (
              <button
                key={lang}
                className={`btn-option ${language === lang ? "active" : ""}`}
                onClick={() => { setLanguage(lang); persistSettings({ language: lang }); }}
              >
                {langLabels[lang]}
              </button>
            ))}
          </div>
        </div>

        {/* Hotkey — configurable global shortcut (v2.7.0) */}
        <div className="settings-section">
          <h3>קיצור מקלדת להפעלה</h3>
          <p className="settings-hint">
            לחץ על השדה ואז על השילוב הרצוי (למשל: Ctrl+Shift+D, Alt+Q, F8). חייב לכלול לפחות מקש פעיל.
          </p>
          <div className="settings-row" style={{ alignItems: "center", gap: 12 }}>
            <input
              type="text"
              readOnly
              value={hotkeyCapturing ? "לחץ על השילוב הרצוי..." : formatHotkey(hotkey)}
              className={`hotkey-input ${hotkeyCapturing ? "capturing" : ""}`}
              onFocus={() => { setHotkeyCapturing(true); setHotkeyError(null); }}
              onBlur={() => setHotkeyCapturing(false)}
              onKeyDown={async (e) => {
                if (!hotkeyCapturing) return;
                e.preventDefault();
                e.stopPropagation();
                const combo = buildComboFromKeyEvent(e.nativeEvent);
                if (!combo) return; // modifier-only press — keep waiting
                try {
                  await applyHotkey(combo);
                  setHotkey(combo);
                  setHotkeyError(null);
                  setHotkeyCapturing(false);
                  (e.target as HTMLInputElement).blur();
                } catch (err) {
                  setHotkeyError(String(err));
                }
              }}
              placeholder="Alt+D"
            />
            <button
              className="btn-secondary"
              onClick={async () => {
                try {
                  await applyHotkey("alt+d");
                  setHotkey("alt+d");
                  setHotkeyError(null);
                } catch (err) {
                  setHotkeyError(String(err));
                }
              }}
            >
              איפוס ל-Alt+D
            </button>
          </div>
          {hotkeyError && <p className="settings-error">{hotkeyError}</p>}
        </div>

        {/* Pause hotkey — separate global shortcut for Pause/Resume (v2.8.0) */}
        <div className="settings-section">
          <h3>קיצור מקלדת להשהיה (Pause)</h3>
          <p className="settings-hint">
            פועל רק בזמן הקלטה פעילה. שימושי כשמכתיבים בתוך Word/דפדפן ורוצים להשהות בלי לעזוב את החלון.
          </p>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={pauseHotkey !== null}
              onChange={async () => {
                const next = pauseHotkey === null ? "alt+p" : null;
                try {
                  await applyPauseHotkey(next);
                  setPauseHotkey(next);
                  setPauseHotkeyError(null);
                } catch (err) {
                  setPauseHotkeyError(String(err));
                }
              }}
            />
            <span className="toggle-text">הפעל קיצור Pause</span>
          </label>
          {pauseHotkey !== null && (
            <div className="settings-row" style={{ alignItems: "center", gap: 12 }}>
              <input
                type="text"
                readOnly
                value={pauseHotkeyCapturing ? "לחץ על השילוב הרצוי..." : formatHotkey(pauseHotkey)}
                className={`hotkey-input ${pauseHotkeyCapturing ? "capturing" : ""}`}
                onFocus={() => { setPauseHotkeyCapturing(true); setPauseHotkeyError(null); }}
                onBlur={() => setPauseHotkeyCapturing(false)}
                onKeyDown={async (e) => {
                  if (!pauseHotkeyCapturing) return;
                  e.preventDefault();
                  e.stopPropagation();
                  const combo = buildComboFromKeyEvent(e.nativeEvent);
                  if (!combo) return;
                  try {
                    await applyPauseHotkey(combo);
                    setPauseHotkey(combo);
                    setPauseHotkeyError(null);
                    setPauseHotkeyCapturing(false);
                    (e.target as HTMLInputElement).blur();
                  } catch (err) {
                    setPauseHotkeyError(String(err));
                  }
                }}
                placeholder="Alt+P"
              />
              <button
                className="btn-secondary"
                onClick={async () => {
                  try {
                    await applyPauseHotkey("alt+p");
                    setPauseHotkey("alt+p");
                    setPauseHotkeyError(null);
                  } catch (err) {
                    setPauseHotkeyError(String(err));
                  }
                }}
              >
                איפוס ל-Alt+P
              </button>
            </div>
          )}
          {pauseHotkeyError && <p className="settings-error">{pauseHotkeyError}</p>}
        </div>

        {/* VAD — toggle + duration slider (v2.7.0) */}
        <div className="settings-section">
          <h3>עצירה אוטומטית בשקט</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={vadEnabled}
              onChange={() => {
                const newVal = !vadEnabled;
                setVadEnabled(newVal);
                invoke("set_vad_enabled", { enabled: newVal }).catch(() => {});
                persistSettings({ vad_enabled: newVal });
              }}
            />
            <span className="toggle-text">
              עצור כשמפסיקים לדבר {vadEnabled ? `(${vadSilenceSecs.toFixed(1)} שניות שקט)` : ""}
            </span>
          </label>
          {vadEnabled && (
            <div className="settings-slider-row">
              <input
                type="range"
                min={1}
                max={10}
                step={0.5}
                value={vadSilenceSecs}
                onChange={(e) => {
                  const v = parseFloat(e.target.value);
                  setVadSilenceSecs(v);
                  applySilenceDuration(v);
                }}
                onMouseUp={() => persistSettings({ vad_silence_secs: vadSilenceSecs })}
                onTouchEnd={() => persistSettings({ vad_silence_secs: vadSilenceSecs })}
              />
              <span className="settings-slider-value">{vadSilenceSecs.toFixed(1)}s</span>
            </div>
          )}
        </div>

        {/* Max recording length — checkbox + slider (v2.7.0) */}
        <div className="settings-section">
          <h3>אורך הקלטה מקסימלי</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={unlimitedRecording}
              onChange={() => {
                const v = !unlimitedRecording;
                setUnlimitedRecording(v);
                // Push the effective value (3600 = 1h ceiling) to the running recorder.
                applyMaxRecording(v ? 3600 : maxRecordingSecs);
                persistSettings({ unlimited_recording: v });
              }}
            />
            <span className="toggle-text">ללא הגבלת זמן</span>
          </label>
          {!unlimitedRecording && (
            <>
              <div className="settings-slider-row">
                <input
                  type="range"
                  min={10}
                  max={600}
                  step={10}
                  value={maxRecordingSecs}
                  onChange={(e) => {
                    const v = parseInt(e.target.value, 10);
                    setMaxRecordingSecs(v);
                    applyMaxRecording(v);
                  }}
                  onMouseUp={() => persistSettings({ max_recording_secs: maxRecordingSecs })}
                  onTouchEnd={() => persistSettings({ max_recording_secs: maxRecordingSecs })}
                />
                <span className="settings-slider-value">{formatDuration(maxRecordingSecs)}</span>
              </div>
              <p className="settings-hint">
                הקלטה תיעצר אוטומטית כשהזמן ייגמר. רלוונטי בעיקר כדי למנוע הקלטות אינסופיות אם שכחת את המיקרופון פתוח.
              </p>
            </>
          )}
          {unlimitedRecording && (
            <p className="settings-hint">
              הקלטות ארוכות עלולות לצרוך הרבה RAM (במצב מקומי) או קרדיט (ב-API). תקרה קשיחה: שעה אחת.
            </p>
          )}
        </div>

        {/* Microphone picker (v2.7.0) */}
        <div className="settings-section">
          <h3>מיקרופון</h3>
          <select
            className="settings-select"
            value={preferredAudioDevice ?? ""}
            onChange={async (e) => {
              const v = e.target.value === "" ? null : e.target.value;
              setPreferredAudioDevice(v);
              await applyPreferredDevice(v);
              persistSettings({ preferred_audio_device: v });
            }}
          >
            <option value="">ברירת מחדל של מערכת ההפעלה</option>
            {devices.map((d) => (
              <option key={d} value={d}>{d}</option>
            ))}
          </select>
          <p className="settings-hint">
            השינוי חל בהקלטה הבאה — הקלטה פעילה לא נקטעת.
          </p>
        </div>

        {/* Streaming — simultaneous transcription via Deepgram WebSocket */}
        <div className="settings-section">
          <h3>הכתבה סימולטנית (ניסיוני)</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={streamingEnabled}
              // Hard mutual-exclusion with Smart Cleanup: when cleanup is on (and this
              // is off) the streaming toggle is LOCKED. Also Deepgram-only. The
              // `&& !streamingEnabled` guard means an already-on toggle stays clickable
              // so a legacy both-on state can self-heal.
              disabled={apiProvider !== "deepgram" || (enhanceEnabled && !streamingEnabled)}
              onChange={() => {
                const v = !streamingEnabled;
                setStreamingEnabled(v);
                persistSettings({ streaming_enabled: v });
              }}
            />
            <span className="toggle-text">
              תמלול בזמן אמת תוך כדי דיבור (Deepgram בלבד, בלי המתנה לעיבוד בסוף)
            </span>
          </label>
          <p className="settings-hint">לא תואם ל-✨ רישוף חכם — כשהרישוף דלוק, ההכתבה הסימולטנית נעולה, וההיפך.</p>
        </div>

        {/* Always on top */}
        <div className="settings-section">
          <h3>חלון מעל הכל</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={alwaysOnTop}
              onChange={() => {
                const v = !alwaysOnTop;
                setAlwaysOnTop(v);
                invoke("set_window_always_on_top", { enabled: v }).catch(() => {});
                persistSettings({ always_on_top: v });
              }}
            />
            <span className="toggle-text">הצג את חלון ההקלטה מעל כל התוכנות (כדי לראות סטטוס)</span>
          </label>
        </div>

        {/* Floating toolbar */}
        <div className="settings-section">
          <h3>פס כלים צף בזמן הקלטה</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={floatingToolbarEnabled}
              onChange={() => {
                const v = !floatingToolbarEnabled;
                setFloatingToolbarEnabled(v);
                persistSettings({ floating_toolbar_enabled: v });
              }}
            />
            <span className="toggle-text">הצג פס מיני תחתון בזמן ההקלטה (מחליף את החלון הראשי)</span>
          </label>
        </div>

        {/* Idle floating button (v2.8.1) */}
        <div className="settings-section">
          <h3>כפתור צף תמידי</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={idleButtonEnabled}
              onChange={() => {
                const v = !idleButtonEnabled;
                setIdleButtonEnabled(v);
                invoke("set_idle_button_enabled", { enabled: v }).catch(() => {});
                persistSettings({ idle_button_enabled: v });
              }}
            />
            <span className="toggle-text">כפתור עגול קטן שתמיד צף — לחיצה אחת מתחילה הכתבה, בלי לזכור קיצור מקלדת</span>
          </label>
          <p className="settings-hint">
            מופיע כשהחלון הראשי מוסתר (למשל אחרי הפעלה אוטומטית בהדלקת המחשב). אפשר לגרור אותו לכל מקום במסך.
          </p>
        </div>

        {/* Audio feedback */}
        <div className="settings-section">
          <h3>צליל בהתחלה ובסיום</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={audioFeedbackEnabled}
              onChange={() => {
                const v = !audioFeedbackEnabled;
                setAudioFeedbackEnabled(v);
                persistSettings({ audio_feedback_enabled: v });
                // Demo the tone the first time the user turns it on so they hear what they're getting.
                if (v) playStartTone();
              }}
            />
            <span className="toggle-text">השמע צליל קצר כשההקלטה מתחילה ומסתיימת</span>
          </label>
          {audioFeedbackEnabled && (
            <>
              <div className="settings-slider-row">
                <input
                  type="range"
                  min={0}
                  max={100}
                  step={5}
                  value={Math.round(audioVolume * 100)}
                  onChange={(e) => {
                    setAudioVolume(parseInt(e.target.value, 10) / 100);
                  }}
                  onMouseUp={() => { persistSettings({ audio_volume: audioVolume }); playStartTone(); }}
                  onTouchEnd={() => { persistSettings({ audio_volume: audioVolume }); playStartTone(); }}
                />
                <span className="settings-slider-value">{Math.round(audioVolume * 100)}%</span>
              </div>
              <p className="settings-hint">עוצמת הצלילים (התחלה, סיום, שגיאה, העתקה).</p>
            </>
          )}
        </div>

        {/* Smart cleanup (רישוף חכם) */}
        <div className="settings-section">
          <h3>✨ רישוף חכם</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={enhanceEnabled}
              // Hard mutual-exclusion with streaming: when streaming is on (and this is
              // off) the cleanup toggle is LOCKED. Also needs a Groq key. The
              // `&& !enhanceEnabled` guard keeps an already-on toggle clickable so a
              // legacy both-on state can self-heal.
              disabled={!hasGroqKey || (streamingEnabled && !enhanceEnabled)}
              onChange={() => {
                const v = !enhanceEnabled;
                setEnhanceEnabled(v);
                persistSettings({ enhance_enabled: v });
              }}
            />
            <span className="toggle-text">נקה מילות מילוי, חזרות ופיסוק מהתמלול לפני ההזרקה (דרך Groq Llama)</span>
          </label>
          <p className="settings-hint">פועל אחרי שמסיימים לדבר (batch). כשההכתבה הסימולטנית דלוקה — הרישוף נעול, וההיפך.</p>

          {/* Dedicated Groq key for cleanup — works regardless of the transcription provider */}
          <p className="settings-hint">מפתח Groq לרישוף (עובד גם כשמתמללים ב-Deepgram או מקומי):</p>
          <div className="api-key-row">
            <input
              type="password"
              className="api-key-input"
              value={groqKey}
              placeholder="gsk_..."
              onChange={(e) => { setGroqKey(e.target.value); setGroqCleanupValid(null); }}
              onBlur={async () => {
                if (groqKey === "••••••••") return;
                try {
                  if (groqKey) await setApiKey("groq", groqKey);
                  else await clearApiKey("groq");
                } catch { /* keep local state either way */ }
              }}
            />
            <button
              className={`btn-test ${groqCleanupValid === true ? "valid" : groqCleanupValid === false ? "invalid" : ""}`}
              onClick={handleTestGroqCleanup}
              disabled={testingGroqCleanup || !groqKey || groqKey === "••••••••"}
            >
              {testingGroqCleanup ? "..." : groqCleanupValid === true ? "✓" : groqCleanupValid === false ? "✗" : "בדוק"}
            </button>
          </div>
          {groqCleanupValid === false && <p className="settings-note error-note">המפתח לא תקין</p>}
          {groqCleanupValid === true && <p className="settings-note success-note">המפתח תקין — הרישוף מוכן</p>}
          <p className="settings-note">
            <a href="https://console.groq.com/keys" target="_blank" rel="noopener" className="link-text">קבל מפתח Groq חינם (ללא כרטיס אשראי) → groq.com</a>
          </p>

          {enhanceEnabled && transcriptionMode === "local" && (
            <p className="settings-hint">⚠️ במצב מקומי: הטקסט המתומלל (לא ההקלטה) יישלח ל-Groq לצורך הרישוף.</p>
          )}
        </div>

        {/* Autostart */}
        <div className="settings-section">
          <h3>הפעלה אוטומטית בהדלקה</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={autostartEnabled}
              onChange={() => {
                const v = !autostartEnabled;
                setAutostartEnabled(v);
                invoke("set_autostart_enabled", { enabled: v }).catch(() => {});
                persistSettings({ autostart_enabled: v });
              }}
            />
            <span className="toggle-text">הפעל את התוכנה אוטומטית כשהמחשב נדלק</span>
          </label>
        </div>

        {/* Mics */}
        <div className="settings-section">
          <h3>מיקרופונים ({devices.length})</h3>
          {devices.length > 0 ? (
            <ul className="device-list">{devices.map((d, i) => <li key={i}>{d}</li>)}</ul>
          ) : (
            <p className="settings-note">לא נמצאו מיקרופונים</p>
          )}
        </div>

        {/* Models */}
        <div className="settings-section">
          <h3>מודלים מקומיים</h3>
          {activeModel && <p className="settings-note active-note">פעיל: <strong>{activeModel}</strong></p>}
          <div className="model-cards">
            {[...models].sort((a, b) => {
              // Surface ivrit-* models first — they're the recommended Hebrew option.
              const aIvrit = a.name.startsWith("ivrit-") ? 0 : 1;
              const bIvrit = b.name.startsWith("ivrit-") ? 0 : 1;
              return aIvrit - bIvrit;
            }).map((m) => {
              const isActive = activeModel === m.name;
              const isDownloading = downloadingModel === m.name;
              const isHebrewModel = m.name.startsWith("ivrit-");
              return (
                <div key={m.name} className={`model-card ${isActive ? "active" : ""} ${m.downloaded ? "downloaded" : ""} ${isHebrewModel ? "hebrew-recommended" : ""}`}>
                  <div className="model-card-header">
                    <span className="model-name">
                      {m.name}
                      {isHebrewModel && <span className="badge-hebrew" title="מודל מותאם לעברית">🇮🇱 מומלץ לעברית</span>}
                      {isActive && <span className="active-dot" />}
                    </span>
                    <span className="model-size">{m.size_label}</span>
                  </div>
                  <p className="model-desc">{m.description}</p>
                  <div className="model-card-actions">
                    {m.downloaded ? (
                      <>
                        <span className="tag-downloaded">מותקן</span>
                        {!isActive && <button onClick={() => loadWhisperModel(m.name)} className="btn-activate" disabled={status === "loading-model"}>הפעל</button>}
                        <button onClick={() => handleDeleteModel(m.name)} className="btn-delete" disabled={isActive && status === "recording"}>מחק</button>
                      </>
                    ) : isDownloading ? (
                      <div className="mini-progress">
                        <div className="progress-bar"><div className="progress-fill" style={{ width: `${downloadProgress}%` }} /></div>
                        <span className="progress-label">{downloadProgress}%</span>
                      </div>
                    ) : (
                      <button onClick={() => handleDownloadModel(m.name)} className="btn-primary btn-small" disabled={status === "downloading"}>הורד</button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* About */}
        <div className="settings-section about-section">
          <h3>אודות</h3>
          <p className="about-app-name">הכתבה בעברית {APP_VERSION}</p>
          <p className="about-brand">BinTech AI — הנרי שטאובר</p>

          <div className="about-cta-grid">
            <a className="about-cta-btn" href={LINKS.youtube} target="_blank" rel="noopener" title="ערוץ YouTube">
              <span>🎥</span><span>YouTube</span>
            </a>
            <a className="about-cta-btn" href={LINKS.whatsappChannel} target="_blank" rel="noopener" title="ערוץ WhatsApp">
              <span>💬</span><span>WhatsApp</span>
            </a>
            <a className="about-cta-btn" href={LINKS.taplink} target="_blank" rel="noopener" title="כל הקישורים">
              <span>🔗</span><span>קישורים</span>
            </a>
            <a className="about-cta-btn about-cta-btn-primary" href={FEEDBACK_URL} title="שלח פידבק במייל">
              <span>✉️</span><span>פידבק</span>
            </a>
          </div>

          <div className="about-meta">
            <a href={LINKS.github} target="_blank" rel="noopener" className="link-text about-meta-link">
              📂 קוד פתוח ב-GitHub
            </a>
            <a href={`mailto:${LINKS.email}`} className="link-text about-meta-link">
              📧 {LINKS.email}
            </a>
          </div>

          <p className="settings-note about-attribution">
            רישיון {APP_LICENSE} · נבנה עם Tauri · whisper.cpp · React · ספריות OSS עם הקרדיט בקוד המקור
          </p>
          <p className="settings-note about-trademark">
            Deepgram ו-Groq הם סימני מסחר של החברות בהתאמה. אין קשר עסקי או חסות.
          </p>
        </div>

        {error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}
      </main>
    );
  }

  // ---- BATCH VIEW ----
  if (view === "batch") {
    const doneCount = batchResults.filter((r) => r.status === "done").length;
    const processingResult = batchActiveResultId != null
      ? batchResults.find((r) => r.id === batchActiveResultId)
      : batchResults[batchCurrentIdx];

    return (
      <main className="container batch-view" dir="rtl">
        {/* Header */}
        <div className="batch-view-header">
          <button className="btn-back" onClick={() => setView("main")} aria-label="חזור">חזור</button>
          <h2 className="batch-view-title">תמלול קובץ</h2>
          <button
            className="btn-settings-labeled"
            style={{ marginInlineStart: "auto" }}
            onClick={() => { setSettingsReturn("batch"); setView("settings"); }}
            title="הגדרות"
            aria-label="הגדרות"
          >
            <span className="gear" aria-hidden="true">⚙</span> הגדרות
          </button>
        </div>

        {/* Engine toggle (cloud/local) — a LIGHT segmented control at the top. Sets the
            engine for REGULAR recording (mic/system). Meeting cards encode their own
            engine, so the toggle dims + disables while a meeting source is selected —
            kept visually lighter than the source cards so the two axes read differently. */}
        {!batchRecording && (() => {
          const meetingSelected = recordSource === "callcloud" || recordSource === "calllocal";
          return (
            <div className={`engine-toggle ${meetingSelected ? "is-disabled" : ""}`} role="group" aria-label="מנוע תמלול">
              <span>מנוע תמלול:</span>
              <div className="engine-toggle-group">
                <button
                  className={`engine-toggle-btn ${batchMode === "cloud" ? "active" : ""}`}
                  onClick={() => !batchRunning && setBatchMode("cloud")}
                  disabled={batchRunning || meetingSelected}
                  aria-pressed={batchMode === "cloud"}
                >☁ ענן</button>
                <button
                  className={`engine-toggle-btn ${batchMode === "local" ? "active" : ""}`}
                  onClick={() => !batchRunning && setBatchMode("local")}
                  disabled={batchRunning || meetingSelected}
                  aria-pressed={batchMode === "local"}
                >💾 מקומי</button>
              </div>
            </div>
          );
        })()}

        {/* batch-source-selector — recording source, chosen before recording, in two
            groups: "הקלטה רגילה" (mic / system) and "פגישות" (callcloud / calllocal).
            System + the whole "פגישות" group are Windows-only (WASAPI loopback) →
            hidden off-Windows. Group headers use inline styles (no new CSS). The git
            grep anchors (calllocal) live in the code below, not the aria-labels. */}
        {!batchRecording && (
          <>
            <div style={{ fontSize: "0.8rem", opacity: 0.7, margin: "8px 0 4px", textAlign: "center" }}>
              הקלטה רגילה
            </div>
            <div className="batch-mode-cards" role="group" aria-label="הקלטה רגילה">
              <button
                className={`batch-mode-card ${recordSource === "mic" ? "active" : ""}`}
                onClick={() => !batchRunning && setRecordSource("mic")}
                disabled={batchRunning}
                aria-pressed={recordSource === "mic"}
              >
                <span className="batch-mode-icon" aria-hidden="true">🎙</span>
                <span className="batch-mode-name">מיקרופון</span>
                <span className="batch-mode-desc">הקול שלכם בלבד</span>
              </button>
              {IS_WINDOWS && (
                <button
                  className={`batch-mode-card ${recordSource === "system" ? "active" : ""}`}
                  onClick={() => !batchRunning && setRecordSource("system")}
                  disabled={batchRunning}
                  aria-pressed={recordSource === "system"}
                >
                  <span className="batch-mode-icon" aria-hidden="true">🔊</span>
                  <span className="batch-mode-name">אודיו מערכת</span>
                  <span className="batch-mode-desc">מה שמתנגן במחשב</span>
                </button>
              )}
            </div>

            {IS_WINDOWS && (
              <>
                <div style={{ fontSize: "0.8rem", opacity: 0.7, margin: "8px 0 4px", textAlign: "center" }}>
                  פגישות <span style={{ opacity: 0.65 }}>· קובעות מנוע בעצמן</span>
                </div>
                <div className="batch-mode-cards" role="group" aria-label="פגישות">
                  <button
                    className={`batch-mode-card ${recordSource === "callcloud" ? "active" : ""}`}
                    onClick={() => !batchRunning && setRecordSource("callcloud")}
                    disabled={batchRunning}
                    aria-pressed={recordSource === "callcloud"}
                  >
                    <span className="batch-mode-icon" aria-hidden="true">📞</span>
                    <span className="batch-mode-name">פגישה — עם זיהוי דוברים</span>
                    <span className="batch-mode-desc">אתם + הצד השני, כל אחד בנפרד · מתומלל בענן</span>
                  </button>
                  <button
                    className={`batch-mode-card ${recordSource === "calllocal" ? "active" : ""}`}
                    onClick={() => !batchRunning && setRecordSource("calllocal")}
                    disabled={batchRunning}
                    aria-pressed={recordSource === "calllocal"}
                  >
                    <span className="batch-mode-icon" aria-hidden="true">🔒</span>
                    <span className="batch-mode-name">פגישה — פרטית במכשיר</span>
                    <span className="batch-mode-desc">אתם + הצד השני יחד · נשאר במחשב, בלי הפרדת דוברים</span>
                  </button>
                </div>
              </>
            )}
          </>
        )}

        {/* Actions — pinned near the top so a growing result list never pushes them off-screen */}
        {!batchRecording && (
          <div className="batch-actions-top">
            <button className="btn-primary batch-pick-btn" onClick={handlePickAndTranscribe} disabled={batchRunning}>
              📁 {batchResults.length === 0 ? "בחר קבצים" : "הוסף קבצים"}
            </button>
            <button className="btn-secondary batch-record-btn" onClick={handleStartBatchRecord} disabled={batchRunning}>
              🎙 הקלט ותמלל
            </button>
          </div>
        )}

        {/* Global error */}
        {batchError && (
          <p className="error batch-error" role="alert" onClick={() => setBatchError("")}>
            ❌ {batchError}
          </p>
        )}

        {/* Empty state */}
        {batchResults.length === 0 && !batchRunning && !batchRecording && (
          <div className="batch-empty-state">
            <div className="batch-empty-icon" aria-hidden="true">🎵</div>
            <p className="batch-empty-title">תמלול קובץ או הקלטה</p>
            <p className="batch-empty-hint">תומך ב-MP3, M4A, WAV, FLAC, OGG, AAC · ניתן לבחור מספר קבצים בבת אחת</p>
          </div>
        )}

        {/* Recording active state */}
        {batchRecording && (
          <div className="batch-record-active" role="status" aria-live="polite">
            <span className="batch-record-dot" aria-hidden="true" />
            <span className="batch-record-timer" aria-label={`זמן הקלטה: ${formatRecordTime(batchRecordElapsed)}`}>
              {formatRecordTime(batchRecordElapsed)}
            </span>
            <span className="batch-record-label">מקליט...</span>
            <div className="batch-record-controls">
              <button className="btn-primary btn-record-stop" onClick={handleStopBatchRecord}>
                ⏹ עצור ותמלל
              </button>
              <button className="btn-secondary btn-record-cancel" onClick={handleCancelBatchRecord}>
                בטל
              </button>
            </div>
          </div>
        )}

        {/* Overall progress */}
        {batchRunning && (
          <div className="batch-overall-progress" role="status" aria-live="polite">
            <div className="batch-overall-row">
              <span className="batch-overall-label">
                מתמלל {batchCurrentIdx + 1} מתוך {batchFileTotal}
                {processingResult?.fileName ? ` — ${processingResult.fileName}` : ""}
              </span>
              <button className="btn-secondary btn-sm" onClick={handleCancelBatch}>בטל</button>
            </div>
            <div className="batch-progress-bar">
              <div
                className="batch-progress-fill"
                style={{ width: `${batchPct}%` }}
                role="progressbar"
                aria-valuenow={batchPct}
                aria-valuemin={0}
                aria-valuemax={100}
              />
            </div>
            <span className="batch-progress-stage">{stageLabel(batchStage)}{batchPct > 0 ? ` ${batchPct}%` : ""}</span>
          </div>
        )}

        {/* File result list */}
        {batchResults.length > 0 && (
          <div className="batch-file-list">
            {batchResults.map((result, idx) => (
              <div key={result.id} className={`batch-file-card batch-file-${result.status}`}>
                <div className="batch-file-header">
                  <span className="batch-file-icon" aria-hidden="true">{result.fileName.startsWith("הקלטה") ? "🎙" : "🎵"}</span>
                  <span className="batch-file-name" title={result.filePath}>{result.fileName}</span>
                  <span className={`batch-file-badge badge-${result.status}`}>
                    {result.status === "pending" && "ממתין"}
                    {result.status === "processing" && (
                      <>
                        <span className="badge-spinner" aria-hidden="true" />
                        {stageLabel(batchStage)}{batchPct > 0 ? ` ${batchPct}%` : ""}
                      </>
                    )}
                    {result.status === "done" && "✅ הושלם"}
                    {result.status === "cancelled" && "⛔ בוטל"}
                    {result.status === "error" && "❌ שגיאה"}
                  </span>
                </div>

                {result.status === "error" && result.error && (
                  <p className="batch-file-error">{result.error}</p>
                )}

                {result.status === "done" && (
                  <div className="batch-file-result">
                    <textarea
                      dir="rtl"
                      className="batch-textarea"
                      value={result.transcript}
                      onChange={(e) => {
                        const val = e.target.value;
                        setBatchResults((prev) =>
                          prev.map((r, i) => i === idx ? { ...r, transcript: val, edited: true } : r)
                        );
                      }}
                      rows={5}
                      aria-label={`תמלול ${result.fileName}`}
                    />
                    <div className="batch-file-actions">
                      <div className="batch-file-actions-row">
                        <button
                          className="btn-secondary btn-sm"
                          onClick={() => injectText(result.transcript)}
                          title="הדבק בשדה הפעיל"
                        >
                          ⌨️ הדבק בחלון הפעיל
                        </button>
                        <button
                          className="btn-secondary btn-sm"
                          onClick={() => navigator.clipboard.writeText(result.transcript)}
                          title="העתק"
                        >
                          📋 העתק
                        </button>
                      </div>
                      <div className="batch-file-actions-row">
                        <button
                          className="btn-secondary btn-sm"
                          onClick={() => exportSingle(result.transcript, "txt", setBatchError)}
                          title="ייצוא מקטע זה כקובץ טקסט"
                        >
                          📄 TXT
                        </button>
                        <button
                          className="btn-secondary btn-sm"
                          onClick={() => exportSingle(result.transcript, "docx", setBatchError)}
                          title="ייצוא מקטע זה כמסמך Word"
                        >
                          📝 Word
                        </button>
                        {isSrtEligible(result) && (
                          <button
                            className="btn-secondary btn-sm"
                            onClick={() => exportSingleSrt(result.segments!, result.transcript, result.isCallCloud, setBatchError)}
                            title="ייצוא כתוביות SRT למקטע זה"
                          >
                            🎬 SRT
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

        {/* Bottom action bar */}
        {!batchRunning && !batchRecording && batchResults.length > 0 && (
          <div className="batch-action-bar">
            {doneCount > 1 && (
              <>
                <span className="batch-export-all-label">ייצוא הכל:</span>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("txt")}>📄 TXT</button>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("docx")}>📝 Word</button>
                {batchResults.filter(isSrtEligible).length > 1 && (
                  <button className="btn-secondary btn-sm" onClick={() => exportBatchSrt()}>🎬 SRT</button>
                )}
              </>
            )}
            <button className="btn-secondary btn-sm batch-clear-btn" onClick={() => setBatchResults([])}>נקה</button>
          </div>
        )}
      </main>
    );
  }

  // ---- MAIN VIEW (compact) ----
  const dismissCloseTip = async () => {
    setShowCloseTip(false);
    await persistSettings({ close_notification_shown: true });
  };

  if (showTermsGate) {
    const termsAccepted = wizardTermsAsIs && wizardTermsKeys;
    const acceptAndClose = async () => {
      try { await invoke("accept_terms"); } catch { /* ok */ }
      setShowTermsGate(false);
    };
    return (
      <main className="container compact" dir="rtl">
        <div className="wizard-step">
          <h2 className="wizard-step-title">עדכון תנאי שימוש — v2.4.0</h2>
          <p className="wizard-subtitle" style={{ marginBottom: "0.6rem" }}>
            בגרסה החדשה הוסרה התלות ב-OpenAI (שמצריכה כרטיס אשראי) ונותרו רק ספקים חינמיים: Deepgram + Groq. נא לאשר את התנאים כדי להמשיך.
          </p>
          <div className="wizard-content" style={{ display: "flex", flexDirection: "column", gap: "0.5rem", textAlign: "right" }}>
            <a href={TERMS_FULL_URL} target="_blank" rel="noopener" className="link-text" style={{ fontSize: "0.85rem" }}>
              לתנאי השימוש המלאים באתר →
            </a>
            <label className="toggle-label" style={{ alignItems: "flex-start", gap: "0.5rem" }}>
              <input type="checkbox" checked={wizardTermsAsIs} onChange={() => setWizardTermsAsIs(!wizardTermsAsIs)} />
              <span className="toggle-text" style={{ fontSize: "0.82rem" }}>
                קראתי והבנתי שהתוכנה ניתנת ״כפי שהיא״ ללא אחריות לדיוק התמלול
              </span>
            </label>
            <label className="toggle-label" style={{ alignItems: "flex-start", gap: "0.5rem" }}>
              <input type="checkbox" checked={wizardTermsKeys} onChange={() => setWizardTermsKeys(!wizardTermsKeys)} />
              <span className="toggle-text" style={{ fontSize: "0.82rem" }}>
                אני מאשר שאני אחראי על מפתחות ה-API שלי ועל השימוש בהם
              </span>
            </label>
          </div>
          <div className="wizard-nav">
            <button
              className="btn-wizard-next"
              onClick={acceptAndClose}
              disabled={!termsAccepted}
              title={!termsAccepted ? "סמנו את שני התנאים כדי להמשיך" : ""}
            >
              {termsAccepted ? "מסכים, המשך" : "סמנו את שני התנאים"}
            </button>
          </div>
        </div>
      </main>
    );
  }

  return (
    <main className="container compact" dir="rtl">
      {showCloseTip && (
        <div className="close-tip-banner">
          <span>💡 Alt+D עובד גם כשהחלון סגור</span>
          <button className="btn-close-tip" onClick={dismissCloseTip}>✓</button>
        </div>
      )}

      {updateAvailable && !updateInstalling && (
        <div className="update-banner">
          <span>🎉 גרסה חדשה {updateAvailable.version} זמינה</span>
          <button
            className="btn-update-install"
            onClick={handleInstallUpdate}
            disabled={status !== "idle"}
            title={status !== "idle" ? "סיים את ההקלטה לפני העדכון" : "התקן עדכון"}
          >
            התקן
          </button>
        </div>
      )}
      {updateInstalling && (
        <div className="update-banner installing">
          <span>⬇ מוריד עדכון {updateProgress}%</span>
          <div className="progress-bar">
            <div className="progress-fill" style={{ width: `${updateProgress}%` }} />
          </div>
        </div>
      )}

      <div className="main-header">
        <div className="main-header-modes">
          <button
            className="btn-batch-nav btn-mode-combined"
            onClick={() => setView("batch")}
            aria-label="הקלט ותמלל או תמלול קבצי שמע"
          >הקלט ותמלל / תמלול קבצי שמע</button>
        </div>
      </div>

      {/* No setup — first-time prompt */}
      {models.length > 0 && downloadedCount === 0 && !apiKeyConfigured && status !== "downloading" && (
        <div className="setup-section compact-setup">
          <p>הגדר מפתח API או הורד מודל בהגדרות ⚙</p>
        </div>
      )}

      {/* Downloading */}
      {status === "downloading" && downloadingModel && (
        <div className="download-section">
          <p>מוריד {downloadingModel}... {downloadProgress}%</p>
          <div className="progress-bar"><div className="progress-fill" style={{ width: `${downloadProgress}%` }} /></div>
        </div>
      )}

      {/* Status */}
      <div className="status-section">
        <div className={`status-indicator ${status} ${showTimeWarning ? "warning" : ""}`}>
          {status === "idle" && (canRecord ? `מוכן — ${langLabels[language]} · ${modeLabel}` : "הגדר API / מודל")}
          {status === "recording" && `🔴 מקליט ${recordingTime.toFixed(0)}s`}
          {status === "transcribing" && "⏳ מתמלל..."}
          {status === "enhancing" && "✨ משכתב..."}
          {status === "downloading" && "מוריד..."}
          {status === "loading-model" && "טוען מודל..."}
          {status === "idle" && modelLoading && "טוען מודל ברקע…"}
        </div>
        {showTimeWarning && <p className="time-warning">נותרו {Math.ceil(timeRemaining)}s</p>}
      </div>

      {/* Record button */}
      <div className="controls">
        <button
          onClick={handleToggleRecording}
          className={`btn-record ${status === "recording" ? "recording" : ""}`}
          disabled={status === "transcribing" || status === "enhancing" || status === "downloading" || status === "loading-model" || !canRecord}
        >
          {status === "recording" ? "⏹ עצור" : "🎤 הכתב"}
        </button>
        <button className="btn-settings-labeled" onClick={() => { setSettingsReturn("main"); setView("settings"); }} title="הגדרות" aria-label="הגדרות"><span className="gear" aria-hidden="true">⚙</span> הגדרות</button>
      </div>

      {/* Recording progress bar — hidden in unlimited mode */}
      {status === "recording" && !unlimitedRecording && (
        <div className="recording-progress">
          <div className="progress-bar">
            <div className={`progress-fill ${showTimeWarning ? "warning" : ""}`} style={{ width: `${(recordingTime / effectiveMaxRecordingSecs) * 100}%` }} />
          </div>
        </div>
      )}

      {/* Live transcription preview — shows while streaming is active */}
      {streamingEnabled && (status === "recording" || status === "transcribing") && livePreview && (
        <div className="live-preview">
          <div className="live-preview-label">תמלול חי</div>
          <div className="live-preview-text" dir="auto">{livePreview}</div>
        </div>
      )}

      {/* Transcript — editable */}
      {transcript && (
        <div className="transcript-section">
          <textarea
            className="transcript-edit"
            value={editableTranscript}
            onChange={(e) => setEditableTranscript(e.target.value)}
            rows={2}
            dir="rtl"
          />
          <div className="transcript-actions">
            <button onClick={() => injectText(editableTranscript)} className="btn-secondary" title="הדבק בשדה הפעיל">
              ⌨️ הדבק בחלון הפעיל
            </button>
            <button onClick={() => navigator.clipboard.writeText(editableTranscript)} className="btn-secondary" title="העתק ללוח">
              📋 העתק
            </button>
          </div>
        </div>
      )}

      {error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}
      {exportNotice && <p className="success-note" style={{ wordBreak: "break-all" }}>{exportNotice}</p>}

      {history.length > 0 && (
        <div className="history-section">
          <div className="history-header">
            <h3>היסטוריה:</h3>
            <div className="history-actions">
              <button
                type="button"
                className="btn-secondary btn-sm"
                onClick={() => exportHistory("txt")}
                disabled={exporting !== null}
                title="ייצוא כקובץ טקסט"
              >
                {exporting === "txt" ? "..." : "📄 TXT"}
              </button>
              <button
                type="button"
                className="btn-secondary btn-sm"
                onClick={() => exportHistory("docx")}
                disabled={exporting !== null}
                title="ייצוא כמסמך Word"
              >
                {exporting === "docx" ? "..." : "📝 Word"}
              </button>
            </div>
          </div>
          {history.length > 1 && history.slice(1).map((h) => (
            <div key={h.id} className="history-item">
              <span className="history-item-text">{h.text}</span>
              <button
                type="button"
                className={`history-copy-btn ${copiedHistoryId === h.id ? "copied" : ""}`}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(h.text);
                    if (audioFeedbackEnabledRef.current) playCopyTone();
                    setCopiedHistoryId(h.id);
                    window.setTimeout(() => {
                      setCopiedHistoryId((cur) => (cur === h.id ? null : cur));
                    }, 1500);
                  } catch { /* clipboard denied */ }
                }}
                title="העתק"
                aria-label="העתק"
              >
                {copiedHistoryId === h.id ? "✓ הועתק" : "📋 העתק"}
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="footer">
        <span>{formatHotkey(hotkey)} · {langLabels[language]} · {vadEnabled ? "עצירה אוטומטית" : "עצירה ידנית"}</span>
        <a href="https://taplink.cc/henry.ai" target="_blank" rel="noopener" className="footer-brand">BinTech AI</a>
      </div>
    </main>
  );
}

export default App;

/**
 * Compact floating toolbar shown during recording (window `toolbar`).
 * Display-only: listens to transcription events for live preview, and emits
 * `hotkey-pressed` when the user clicks stop so the main window handles it.
 */
export function ToolbarApp() {
  const [livePreview, setLivePreview] = useState("");
  const [paused, setPaused] = useState(false);
  const [audioLevel, setAudioLevel] = useState(0);
  // "recording" = full bar; "idle" = small floating circle (one click to
  // start dictation). Backend emits `toolbar-mode` to switch between them.
  const [mode, setMode] = useState<"idle" | "recording">("recording");
  const [vadState, setVadState] = useState<VadStatePayload>({
    state: "speaking",
    silent_secs: 0,
    silence_total: 0,
    vad_off: false,
  });
  const liveFinalRef = useRef("");
  // Click-vs-drag discrimination for the idle circle: remember the mousedown
  // screen point; movement beyond ~5px = drag, otherwise = click (start rec).
  const idleDownRef = useRef<{ x: number; y: number } | null>(null);
  const idleDraggingRef = useRef(false);
  // Pending single-click timer — lets us tell a single click (start dictation)
  // from a double click (open the full window).
  const idleClickTimerRef = useRef<number | null>(null);

  // Mount-time mode detection. The backend emits `toolbar-mode` when it shows
  // the window, but on a cold autostart-minimized launch that emit can fire
  // before this webview has attached its listener. The window's own size is
  // the source of truth: a ~56px window is the idle circle.
  useEffect(() => {
    const win = getCurrentWindow();
    win
      .innerSize()
      .then(async (sz) => {
        const sf = await win.scaleFactor().catch(() => 1);
        const logicalW = sz.width / (sf || 1);
        setMode(logicalW < 100 ? "idle" : "recording");
      })
      .catch(() => {});
    return () => {
      if (idleClickTimerRef.current !== null) {
        window.clearTimeout(idleClickTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    const unlistenInterim = listen<InterimPayload>("transcription-interim", (event) => {
      const { text, is_final } = event.payload;
      if (!text) return;
      if (is_final) {
        liveFinalRef.current = liveFinalRef.current
          ? `${liveFinalRef.current} ${text}`
          : text;
        setLivePreview(liveFinalRef.current);
      } else {
        setLivePreview(
          liveFinalRef.current ? `${liveFinalRef.current} ${text}` : text
        );
      }
    });
    const unlistenReset = listen("toolbar-reset", () => {
      liveFinalRef.current = "";
      setLivePreview("");
      setPaused(false);
      setAudioLevel(0);
    });
    const unlistenLevel = listen<number>("audio-level", (event) => {
      setAudioLevel(event.payload);
    });
    const unlistenVad = listen<VadStatePayload>("vad-state", (event) => {
      setVadState(event.payload);
    });
    const unlistenMode = listen<string>("toolbar-mode", (event) => {
      setMode(event.payload === "idle" ? "idle" : "recording");
    });

    // Persist drag position so the toolbar reappears where the user left it.
    // Debounced so we don't hammer the disk on every pixel of a drag.
    let saveTimer: number | null = null;
    let scaleFactor = 1;
    const tauriWindow = getCurrentWindow();
    tauriWindow.scaleFactor().then((s) => { scaleFactor = s || 1; }).catch(() => {});
    const unlistenMove = tauriWindow.onMoved(({ payload }) => {
      // payload is in physical pixels; convert to logical for cross-DPI consistency.
      const logicalX = payload.x / scaleFactor;
      const logicalY = payload.y / scaleFactor;
      if (saveTimer !== null) window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(() => {
        invoke("set_toolbar_position", { x: logicalX, y: logicalY }).catch(() => {});
        saveTimer = null;
      }, 500);
    });

    return () => {
      unlistenInterim.then((fn) => fn());
      unlistenReset.then((fn) => fn());
      unlistenLevel.then((fn) => fn());
      unlistenVad.then((fn) => fn());
      unlistenMode.then((fn) => fn());
      unlistenMove.then((fn) => fn());
      if (saveTimer !== null) window.clearTimeout(saveTimer);
    };
  }, []);

  const handleStop = useCallback(async () => {
    // 1) Re-use main's hotkey handler — it already toggles recording state and
    //    runs the full transcribe → inject → history pipeline.
    await emit("hotkey-pressed", "toolbar");
    // 2) Safety fallback — if main's listener no-ops (e.g. status was neither
    //    "recording" nor "idle" at click time, like a status flicker between
    //    paths), the toolbar would have stayed visible forever. Force-hide it
    //    after a short window. If main already handled the click, this call
    //    is a no-op (window is already hidden).
    setTimeout(() => {
      invoke("hide_toolbar_window", { forceShowMain: true }).catch(() => {});
    }, 400);
  }, []);

  // Mouse-down drag handler for the toolbar window.
  //
  // `data-tauri-drag-region` alone doesn't reliably work on transparent +
  // focus:false windows on Windows (the OS routes mouse messages differently
  // when the window can't take focus). The Window.startDragging() API works
  // regardless of focus state — it invokes the OS-level drag loop directly.
  //
  // We skip the drag if the user clicked on a button (so pause/stop still
  // fire), and we don't preventDefault on those targets so React onClick
  // continues to fire.
  const handleDragMouseDown = useCallback(async (e: React.MouseEvent) => {
    if (e.button !== 0) return; // left button only
    const target = e.target as HTMLElement;
    if (target.tagName === "BUTTON" || target.closest("button")) return;
    e.preventDefault();
    try { await getCurrentWindow().startDragging(); } catch { /* ok */ }
  }, []);

  // Idle circle interaction. We DON'T call startDragging() on mousedown
  // (that hands the OS the mouse and we'd never tell a click from a drag).
  // Instead: track the down point, start the OS drag only once movement
  // exceeds ~5px, and treat a release with no drag as a click → start rec.
  const handleIdleMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0) return;
    idleDownRef.current = { x: e.screenX, y: e.screenY };
    idleDraggingRef.current = false;
  }, []);

  const handleIdleMouseMove = useCallback(async (e: React.MouseEvent) => {
    const start = idleDownRef.current;
    if (!start || idleDraggingRef.current) return;
    if (Math.hypot(e.screenX - start.x, e.screenY - start.y) > 5) {
      idleDraggingRef.current = true;
      try { await getCurrentWindow().startDragging(); } catch { /* ok */ }
    }
  }, []);

  const handleIdleMouseUp = useCallback(async () => {
    const start = idleDownRef.current;
    idleDownRef.current = null;
    if (!start) return;
    if (idleDraggingRef.current) { idleDraggingRef.current = false; return; }
    // Second click within the window → double-click → open the full window.
    if (idleClickTimerRef.current !== null) {
      window.clearTimeout(idleClickTimerRef.current);
      idleClickTimerRef.current = null;
      await invoke("open_main_window").catch(() => {});
      return;
    }
    // First click → wait briefly; if no second click lands, it's a single
    // click → start dictation (reuse main's hotkey handler: status "idle" →
    // beginRecording → show_toolbar_window swaps this window to the bar).
    idleClickTimerRef.current = window.setTimeout(() => {
      idleClickTimerRef.current = null;
      emit("hotkey-pressed", "toolbar").catch(() => {});
    }, 240);
  }, []);

  // Right-click the circle also opens the full window (power-user shortcut).
  const handleIdleContextMenu = useCallback(async (e: React.MouseEvent) => {
    e.preventDefault();
    idleDownRef.current = null;
    if (idleClickTimerRef.current !== null) {
      window.clearTimeout(idleClickTimerRef.current);
      idleClickTimerRef.current = null;
    }
    await invoke("open_main_window").catch(() => {});
  }, []);

  const handlePauseToggle = useCallback(async () => {
    try {
      if (paused) {
        await invoke("resume_recording");
        setPaused(false);
      } else {
        await invoke("pause_recording");
        setPaused(true);
      }
    } catch { /* keep state in sync if backend rejected */ }
  }, [paused]);

  // VAD indicator label — mirrors backend states.
  let vadLabel: string;
  if (vadState.vad_off) {
    vadLabel = "VAD כבוי";
  } else if (vadState.state === "silent" && vadState.silence_total > 0) {
    const remaining = Math.max(0, vadState.silence_total - vadState.silent_secs);
    vadLabel = `💤 ${remaining.toFixed(1)} שנ׳`;
  } else {
    vadLabel = "🎙 מאזין";
  }

  // Clamp + percent for the volume bar; while paused, show empty.
  const levelPct = paused ? 0 : Math.max(0, Math.min(1, audioLevel)) * 100;

  if (mode === "idle") {
    return (
      <div
        className="toolbar-idle"
        role="button"
        tabIndex={0}
        dir="rtl"
        title="לחיצה — התחל הכתבה · דאבל-קליק — פתח חלון · גרירה — הזז"
        aria-label="התחל הכתבה (לחיצה כפולה פותחת את החלון המלא)"
        onMouseDown={handleIdleMouseDown}
        onMouseMove={handleIdleMouseMove}
        onMouseUp={handleIdleMouseUp}
        onContextMenu={handleIdleContextMenu}
      >
        <img className="toolbar-idle-logo" src="/app-icon.png" alt="" draggable={false} />
      </div>
    );
  }

  return (
    <div
      className={`toolbar-view ${paused ? "paused" : ""}`}
      dir="rtl"
      onMouseDown={handleDragMouseDown}
      title="גרור כדי להזיז"
    >
      {/* Top row — recording dot + level meter + VAD indicator + buttons.
          The whole bar is a drag handle except for the buttons (handled in
          handleDragMouseDown via closest('button')). */}
      <div className="toolbar-row-top">
        <span className="toolbar-dot" aria-hidden="true" />
        <div className="toolbar-meter" aria-hidden="true">
          <div className="toolbar-meter-fill" style={{ width: `${levelPct}%` }} />
        </div>
        <span
          className={`toolbar-vad ${vadState.vad_off ? "off" : vadState.state}`}
          title={vadState.vad_off ? "עצירה אוטומטית כבויה" : ""}
        >
          {vadLabel}
        </span>
        <button
          type="button"
          className="toolbar-pause"
          onClick={handlePauseToggle}
          title={paused ? "המשך" : "השהה"}
          aria-label={paused ? "המשך" : "השהה"}
        >
          {paused ? "▶" : "⏸"}
        </button>
        <button
          type="button"
          className="toolbar-stop"
          onClick={handleStop}
          title="עצור"
          aria-label="עצור"
        >
          ⏹
        </button>
      </div>
      {/* Bottom row — live transcription preview (or status text). */}
      <div className="toolbar-row-bottom">
        <span className="toolbar-live" dir="auto">
          {paused ? "⏸ הושהה" : (livePreview || "מקליט...")}
        </span>
      </div>
    </div>
  );
}

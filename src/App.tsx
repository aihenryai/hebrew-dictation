import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import "./App.css";

/* ----------- אפליקציה: קבועים ----------- */
const APP_VERSION = "v2.4.0";
const APP_LICENSE = "MIT";

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

type AppStatus = "idle" | "recording" | "transcribing" | "downloading" | "loading-model";
type AppView = "main" | "settings" | "onboarding";
type Language = "he" | "en" | "multi" | "auto";
type TranscriptionMode = "api" | "local" | "auto_fallback";
type ApiProvider = "deepgram" | "groq";

/** Settings sent to the backend (keys only included when user explicitly changes them). */
interface AppSettings {
  transcription_mode: TranscriptionMode;
  api_provider: ApiProvider;
  deepgram_api_key: string | null;
  groq_api_key: string | null;
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

let historyIdCounter = 0;
function App() {
  const [status, setStatus] = useState<AppStatus>("idle");
  const [view, setView] = useState<AppView>("main");
  const [transcript, setTranscript] = useState("");
  const [editableTranscript, setEditableTranscript] = useState("");
  const [history, setHistory] = useState<{ id: number; text: string }[]>([]);
  const [whisperLoaded, setWhisperLoaded] = useState(false);
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
  const [livePreview, setLivePreview] = useState("");
  const [copiedHistoryId, setCopiedHistoryId] = useState<number | null>(null);
  const [updateAvailable, setUpdateAvailable] = useState<{ version: string } | null>(null);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateProgress, setUpdateProgress] = useState(0);
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
  useEffect(() => { vadEnabledRef.current = vadEnabled; }, [vadEnabled]);
  useEffect(() => { languageRef.current = language; }, [language]);
  useEffect(() => { transcriptionModeRef.current = transcriptionMode; }, [transcriptionMode]);
  useEffect(() => { streamingEnabledRef.current = streamingEnabled; }, [streamingEnabled]);

  const getMaxRecordingSecs = () => transcriptionModeRef.current === "local" ? MAX_RECORDING_LOCAL : MAX_RECORDING_API;

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

  const stopAndTranscribe = useCallback(async () => {
    if (statusRef.current !== "recording") return;

    setStatus("transcribing");
    stopVadPolling();
    stopTimer();

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
          setHistory((prev) => [{ id: ++historyIdCounter, text }, ...prev].slice(0, 20));
        }
      } else {
        const samples = await invoke("stop_recording") as number[];
        if (samples.length < MIN_TRANSCRIBE_SAMPLES) {
          setStatus("idle");
          return;
        }

        const text = await invoke("transcribe", { samples, language: languageRef.current }) as string;
        if (text && text.trim()) {
          setTranscript(text);
          setEditableTranscript(text);
          setHistory((prev) => [{ id: ++historyIdCounter, text }, ...prev].slice(0, 20));
          // Auto-inject into focused field
          await injectText(text);
        }
      }
    } catch (e) {
      setError(String(e));
    }
    await invoke("hide_toolbar_window").catch(() => {});
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
    const unlistenHotkey = listen("hotkey-pressed", async () => {
      const currentStatus = statusRef.current;
      if (currentStatus === "recording") {
        stopAndTranscribe();
      } else if (currentStatus === "idle") {
        await beginRecording();
      }
    });
    return () => { unlistenHotkey.then((fn) => fn()); };
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
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenClose.then((fn) => fn());
      unlistenFocus.then((fn) => fn());
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
      setApiProvider(settings.api_provider);
      setLanguage(settings.language as Language);
      setVadEnabled(settings.vad_enabled);
      // Keys are redacted — just track whether they exist on the backend.
      if (settings.has_deepgram_key) setDeepgramKey("••••••••");
      if (settings.has_groq_key) setGroqKey("••••••••");
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
      await loadWhisperModel(anyDownloaded.name);
    }
  }

  /** Send null for keys unless user typed a real new value (not the placeholder). */
  const sanitizeKey = (key: string | null): string | null => {
    if (!key || key === "••••••••") return null;
    return key;
  };

  const persistSettings = useCallback(async (overrides: Partial<AppSettings> = {}) => {
    const settings: AppSettings = {
      transcription_mode: transcriptionMode,
      api_provider: apiProvider,
      deepgram_api_key: sanitizeKey(deepgramKey),
      groq_api_key: sanitizeKey(groqKey),
      preferred_model: selectedModel,
      language: language,
      vad_enabled: vadEnabled,
      always_on_top: alwaysOnTop,
      autostart_enabled: autostartEnabled,
      streaming_enabled: streamingEnabled,
      floating_toolbar_enabled: floatingToolbarEnabled,
      ...overrides,
    };
    // Also sanitize keys in overrides
    if (settings.deepgram_api_key === "••••••••") settings.deepgram_api_key = null;
    if (settings.groq_api_key === "••••••••") settings.groq_api_key = null;
    try { await invoke("update_settings", { newSettings: settings }); } catch { /* ok */ }
  }, [transcriptionMode, apiProvider, deepgramKey, groqKey, selectedModel, language, vadEnabled, alwaysOnTop, autostartEnabled, streamingEnabled, floatingToolbarEnabled]);

  async function handleTestApiKey() {
    const activeKey = apiProvider === "groq" ? groqKey : deepgramKey;
    if (!activeKey) return;
    setTestingApiKey(true);
    setApiKeyValid(null);
    try {
      await invoke("test_api_key", { provider: apiProvider, apiKey: activeKey });
      setApiKeyValid(true);
    } catch { setApiKeyValid(false); }
    setTestingApiKey(false);
  }

  async function loadDevices() {
    try {
      const devs = await invoke("get_audio_devices");
      setDevices(devs as string[]);
    } catch (e) { setError(String(e)); }
  }

  async function loadWhisperModel(modelName?: string) {
    const name = modelName || selectedModel;
    setStatus("loading-model");
    try {
      await invoke("load_whisper_model", { modelName: name });
      setWhisperLoaded(true);
      setActiveModel(name);
      setSelectedModel(name);
      setStatus("idle");
    } catch (e) { setError(String(e)); setStatus("idle"); }
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
  const maxRecordingSecs = transcriptionMode === "local" ? MAX_RECORDING_LOCAL : MAX_RECORDING_API;
  const timeRemaining = maxRecordingSecs - recordingTime;
  const showTimeWarning = status === "recording" && timeRemaining <= 10;
  const activeApiKey = apiProvider === "groq" ? groqKey : deepgramKey;
  const apiKeyConfigured = transcriptionMode !== "local" && activeApiKey.length > 0;
  const canRecord = whisperLoaded || apiKeyConfigured;
  const langLabels: Record<Language, string> = { he: "עברית", en: "English", multi: "עברית + אנגלית", auto: "אוטומטי" };
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
      if (wizardChoice === "api" && wizardApiKey) {
        setDeepgramKey(wizardApiKey);
        setApiProvider("deepgram");
        setTranscriptionMode("api");
        await persistSettings({
          onboarding_completed: true,
          deepgram_api_key: wizardApiKey,
          api_provider: "deepgram",
          transcription_mode: "api",
        });
      } else if (wizardChoice === "groq" && wizardApiKey) {
        setGroqKey(wizardApiKey);
        setApiProvider("groq");
        setTranscriptionMode("api");
        setStreamingEnabled(false);
        await persistSettings({
          onboarding_completed: true,
          groq_api_key: wizardApiKey,
          api_provider: "groq",
          transcription_mode: "api",
          streaming_enabled: false,
        });
      } else if (wizardChoice === "local") {
        setTranscriptionMode("local");
        await persistSettings({ onboarding_completed: true, transcription_mode: "local" });
      } else {
        await persistSettings({ onboarding_completed: true });
      }
      try { await invoke("accept_terms"); } catch { /* ok */ }
      setView("main");
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
          <button className="btn-back" onClick={() => setView("main")}>חזור</button>
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
                onBlur={() => persistSettings(
                  apiProvider === "groq"
                    ? { groq_api_key: groqKey || null }
                    : { deepgram_api_key: deepgramKey || null }
                )}
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
            {(["he", "en", "multi", "auto"] as Language[]).map((lang) => (
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

        {/* VAD */}
        <div className="settings-section">
          <h3>עצירה אוטומטית</h3>
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
            <span className="toggle-text">עצור כשמפסיקים לדבר (4.5 שניות שקט)</span>
          </label>
        </div>

        {/* Streaming — simultaneous transcription via Deepgram WebSocket */}
        <div className="settings-section">
          <h3>הכתבה סימולטנית (ניסיוני)</h3>
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={streamingEnabled}
              disabled={apiProvider !== "deepgram"}
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
            {models.map((m) => {
              const isActive = activeModel === m.name;
              const isDownloading = downloadingModel === m.name;
              return (
                <div key={m.name} className={`model-card ${isActive ? "active" : ""} ${m.downloaded ? "downloaded" : ""}`}>
                  <div className="model-card-header">
                    <span className="model-name">{m.name}{isActive && <span className="active-dot" />}</span>
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
            Deepgram, OpenAI ו-Groq הם סימני מסחר של החברות בהתאמה. אין קשר עסקי או חסות.
          </p>
        </div>

        {error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}
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
        <h1>🎤 הכתבה</h1>
        <button className="btn-settings" onClick={() => setView("settings")} title="הגדרות">⚙</button>
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
          {status === "downloading" && "מוריד..."}
          {status === "loading-model" && "טוען מודל..."}
        </div>
        {showTimeWarning && <p className="time-warning">נותרו {Math.ceil(timeRemaining)}s</p>}
      </div>

      {/* Record button */}
      <div className="controls">
        <button
          onClick={handleToggleRecording}
          className={`btn-record ${status === "recording" ? "recording" : ""}`}
          disabled={status === "transcribing" || status === "downloading" || status === "loading-model" || !canRecord}
        >
          {status === "recording" ? "⏹ עצור" : "🎤 הקלט"}
        </button>
      </div>

      {/* Recording progress bar */}
      {status === "recording" && (
        <div className="recording-progress">
          <div className="progress-bar">
            <div className={`progress-fill ${showTimeWarning ? "warning" : ""}`} style={{ width: `${(recordingTime / maxRecordingSecs) * 100}%` }} />
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
              ⌨️ הדבק
            </button>
            <button onClick={() => navigator.clipboard.writeText(editableTranscript)} className="btn-secondary" title="העתק ללוח">
              📋 העתק
            </button>
          </div>
        </div>
      )}

      {error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}

      {history.length > 1 && (
        <div className="history-section">
          <h3>היסטוריה:</h3>
          {history.slice(1).map((h) => (
            <div key={h.id} className="history-item">
              <span className="history-item-text">{h.text}</span>
              <button
                type="button"
                className={`history-copy-btn ${copiedHistoryId === h.id ? "copied" : ""}`}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(h.text);
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
        <span>Alt+D · {langLabels[language]} · {vadEnabled ? "אוטומטי" : "ידני"}</span>
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
  const liveFinalRef = useRef("");

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
    });
    return () => {
      unlistenInterim.then((fn) => fn());
      unlistenReset.then((fn) => fn());
    };
  }, []);

  const handleStop = useCallback(async () => {
    // Re-use main's hotkey handler — it already toggles recording state.
    await emit("hotkey-pressed", "toolbar");
  }, []);

  return (
    <div className="toolbar-view" dir="rtl">
      <span className="toolbar-dot" aria-hidden="true" />
      <span className="toolbar-live" dir="auto">
        {livePreview || "מקליט..."}
      </span>
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
  );
}

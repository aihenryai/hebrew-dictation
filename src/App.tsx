import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type AppStatus = "idle" | "recording" | "transcribing" | "downloading" | "loading-model";
type AppView = "main" | "settings" | "onboarding";
type Language = "he" | "en" | "auto";
type TranscriptionMode = "api" | "local" | "auto_fallback";
type ApiProvider = "open_ai" | "deepgram";

interface AppSettings {
  transcription_mode: TranscriptionMode;
  api_provider: ApiProvider;
  openai_api_key: string | null;
  deepgram_api_key: string | null;
  preferred_model: string;
  language: string;
  vad_enabled: boolean;
  onboarding_completed?: boolean;
  close_notification_shown?: boolean;
}

interface ModelInfo {
  name: string;
  size_bytes: number;
  size_label: string;
  downloaded: boolean;
  description: string;
}

const MIN_TRANSCRIBE_SAMPLES = 8000;
const MAX_RECORDING_LOCAL = 60;   // 1 minute for local whisper
const MAX_RECORDING_API = 300;    // 5 minutes for API

let historyIdCounter = 0;

function App() {
  const [status, setStatus] = useState<AppStatus>("idle");
  const [view, setView] = useState<AppView>("main");
  const [transcript, setTranscript] = useState("");
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
  const [openaiKey, setOpenaiKey] = useState("");
  const [deepgramKey, setDeepgramKey] = useState("");
  const [apiKeyValid, setApiKeyValid] = useState<boolean | null>(null);
  const [testingApiKey, setTestingApiKey] = useState(false);
  const [wizardStep, setWizardStep] = useState(1);
  const [wizardApiKey, setWizardApiKey] = useState("");
  const [wizardKeyValid, setWizardKeyValid] = useState<boolean | null>(null);
  const [wizardKeyTesting, setWizardKeyTesting] = useState(false);
  const [wizardChoice, setWizardChoice] = useState<"api" | "local" | null>(null);
  const [showCloseTip, setShowCloseTip] = useState(false);
  const pendingCloseTipRef = useRef(false);
  const statusRef = useRef(status);
  const vadEnabledRef = useRef(vadEnabled);
  const languageRef = useRef(language);
  const vadPollRef = useRef<number | null>(null);
  const timerRef = useRef<number | null>(null);
  const transcriptionModeRef = useRef(transcriptionMode);

  useEffect(() => { statusRef.current = status; }, [status]);
  useEffect(() => { vadEnabledRef.current = vadEnabled; }, [vadEnabled]);
  useEffect(() => { languageRef.current = language; }, [language]);
  useEffect(() => { transcriptionModeRef.current = transcriptionMode; }, [transcriptionMode]);

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

  const stopAndTranscribe = useCallback(async () => {
    if (statusRef.current !== "recording") return;

    setStatus("transcribing");
    stopVadPolling();
    stopTimer();

    try {
      const samples = await invoke("stop_recording") as number[];
      if (samples.length < MIN_TRANSCRIBE_SAMPLES) {
        setStatus("idle");
        return;
      }

      const text = await invoke("transcribe", { samples, language: languageRef.current }) as string;
      if (text && text.trim()) {
        setTranscript(text);
        setHistory((prev) => [{ id: ++historyIdCounter, text }, ...prev].slice(0, 20));
        try {
          await invoke("inject_text", { text });
        } catch {
          // Inject may fail if no text field is focused
        }
      }
    } catch (e) {
      setError(String(e));
    }
    setStatus("idle");
    setRecordingTime(0);
  }, [stopVadPolling, stopTimer]);

  const startVadPolling = useCallback(() => {
    if (vadPollRef.current) return;
    vadPollRef.current = window.setInterval(async () => {
      try {
        const silenceDetected = await invoke("check_silence") as boolean;
        const timeoutReached = await invoke("check_timeout") as boolean;
        if ((silenceDetected || timeoutReached) && statusRef.current === "recording") {
          stopAndTranscribe();
        }
      } catch {
        // Ignore polling errors
      }
    }, 150);
  }, [stopAndTranscribe]);

  const startTimer = useCallback(() => {
    setRecordingTime(0);
    timerRef.current = window.setInterval(() => {
      setRecordingTime((prev) => prev + 0.1);
    }, 100);
  }, []);

  // Hotkey handler
  useEffect(() => {
    const unlistenHotkey = listen("hotkey-pressed", async () => {
      const currentStatus = statusRef.current;
      if (currentStatus === "recording") {
        stopAndTranscribe();
      } else if (currentStatus === "idle") {
        setError("");
        try {
          await invoke("set_vad_enabled", { enabled: vadEnabledRef.current });
          await invoke("set_max_recording_secs", { secs: getMaxRecordingSecs() });
          await invoke("start_recording");
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
      }
    });

    return () => { unlistenHotkey.then((fn) => fn()); };
  }, []);

  const handleToggleRecording = useCallback(async () => {
    const currentStatus = statusRef.current;
    if (currentStatus === "recording") {
      await stopAndTranscribe();
    } else if (currentStatus === "idle") {
      setError("");
      try {
        await invoke("set_vad_enabled", { enabled: vadEnabled });
        await invoke("set_max_recording_secs", { secs: getMaxRecordingSecs() });
        await invoke("start_recording");
        setStatus("recording");
        startTimer();
        startVadPolling();
      } catch (e) {
        setError(String(e));
      }
    }
  }, [vadEnabled, stopAndTranscribe, startVadPolling, startTimer]);

  // Init
  async function refreshModels() {
    try {
      const allModels = await invoke("get_all_models_status") as ModelInfo[];
      setModels(allModels);
      return allModels;
    } catch {
      return [];
    }
  }

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
      const settings = await invoke("get_settings") as AppSettings;
      setTranscriptionMode(settings.transcription_mode);
      setApiProvider(settings.api_provider);
      setLanguage(settings.language as Language);
      setVadEnabled(settings.vad_enabled);
      if (settings.openai_api_key) setOpenaiKey(settings.openai_api_key);
      if (settings.deepgram_api_key) setDeepgramKey(settings.deepgram_api_key);
      if (settings.preferred_model) {
        preferredModelName = settings.preferred_model;
        setSelectedModel(preferredModelName);
      }
      needsOnboarding = !settings.onboarding_completed;
    } catch {
      // Use defaults
    }

    if (needsOnboarding) {
      setView("onboarding");
    }

    const allModels = await refreshModels();
    const preferred = allModels.find((m) => m.name === preferredModelName && m.downloaded);
    const anyDownloaded = preferred || allModels.find((m) => m.downloaded);
    if (anyDownloaded) {
      setSelectedModel(anyDownloaded.name);
      await loadWhisperModel(anyDownloaded.name);
    }
  }

  const persistSettings = useCallback(async (overrides: Partial<AppSettings> = {}) => {
    const settings: AppSettings = {
      transcription_mode: transcriptionMode,
      api_provider: apiProvider,
      openai_api_key: openaiKey || null,
      deepgram_api_key: deepgramKey || null,
      preferred_model: selectedModel,
      language: language,
      vad_enabled: vadEnabled,
      ...overrides,
    };
    try {
      await invoke("update_settings", { newSettings: settings });
    } catch {
      // Settings save failure is non-critical
    }
  }, [transcriptionMode, apiProvider, openaiKey, deepgramKey, selectedModel, language, vadEnabled]);

  async function handleTestApiKey() {
    const activeKey = apiProvider === "open_ai" ? openaiKey : deepgramKey;
    if (!activeKey) return;
    setTestingApiKey(true);
    setApiKeyValid(null);
    try {
      await invoke("test_api_key", { provider: apiProvider, apiKey: activeKey });
      setApiKeyValid(true);
    } catch {
      setApiKeyValid(false);
    }
    setTestingApiKey(false);
  }

  async function loadDevices() {
    try {
      const devs = await invoke("get_audio_devices");
      setDevices(devs as string[]);
    } catch (e) {
      setError(String(e));
    }
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
    } catch (e) {
      setError(String(e));
      setStatus("idle");
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
      if (!activeModel) {
        await loadWhisperModel(modelName);
      }
    } catch (e) {
      setError(String(e));
      setStatus("idle");
      setDownloadingModel(null);
    }
  }

  async function handleDeleteModel(modelName: string) {
    setError("");
    try {
      await invoke("delete_model", { modelName });
      if (activeModel === modelName) {
        setActiveModel(null);
        setWhisperLoaded(false);
      }
      await refreshModels();
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleActivateModel(modelName: string) {
    await loadWhisperModel(modelName);
  }

  // Computed values
  const downloadedCount = models.filter((m) => m.downloaded).length;
  const maxRecordingSecs = transcriptionMode === "local" ? MAX_RECORDING_LOCAL : MAX_RECORDING_API;
  const timeRemaining = maxRecordingSecs - recordingTime;
  const showTimeWarning = status === "recording" && timeRemaining <= 15;
  const activeApiKey = apiProvider === "open_ai" ? openaiKey : deepgramKey;
  const apiKeyConfigured = transcriptionMode !== "local" && activeApiKey.length > 0;
  const canRecord = whisperLoaded || apiKeyConfigured;

  const langLabels: Record<Language, string> = {
    he: "עברית",
    en: "English",
    auto: "זיהוי אוטומטי",
  };

  const modeLabel = transcriptionMode === "api" ? "API" : transcriptionMode === "local" ? "מקומי" : "אוטומטי";
  const statusLabels: Record<AppStatus, string> = {
    idle: canRecord ? `מוכן — Alt+D | ${langLabels[language]} | ${modeLabel}` : "מוכן",
    recording: `🔴 מקליט... ${recordingTime.toFixed(1)}/${maxRecordingSecs}s`,
    transcribing: transcriptionMode !== "local" && apiKeyConfigured
      ? `⏳ מתמלל (${apiProvider === "deepgram" ? "Deepgram" : "OpenAI"})...`
      : "⏳ מתמלל...",
    downloading: "מוריד מודל...",
    "loading-model": "טוען מודל...",
  };

  // ---- ONBOARDING WIZARD ----
  if (view === "onboarding") {
    const handleWizardTestKey = async () => {
      if (!wizardApiKey) return;
      setWizardKeyTesting(true);
      setWizardKeyValid(null);
      try {
        await invoke("test_api_key", { provider: "deepgram" as ApiProvider, apiKey: wizardApiKey });
        setWizardKeyValid(true);
      } catch {
        setWizardKeyValid(false);
      }
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
      } else if (wizardChoice === "local") {
        setTranscriptionMode("local");
        await persistSettings({
          onboarding_completed: true,
          transcription_mode: "local",
        });
      } else {
        await persistSettings({ onboarding_completed: true });
      }
      setView("main");
    };

    return (
      <main className="container" dir="rtl">
        <div className="wizard-dots">
          {[1, 2, 3].map((s) => (
            <span key={s} className={`wizard-dot ${wizardStep === s ? "active" : wizardStep > s ? "done" : ""}`} />
          ))}
        </div>

        {wizardStep === 1 && (
          <div className="wizard-step">
            <div className="wizard-icon">🎤</div>
            <h1 className="wizard-title">הכתבה בעברית</h1>
            <p className="wizard-subtitle">by BinTech AI — קוד פתוח</p>
            <div className="wizard-content">
              <p>הכתבה קולית בעברית מכל מקום במחשב.</p>
              <div className="wizard-highlight">
                <span className="wizard-key">Alt + D</span>
                <span>להקלטה ועצירה</span>
              </div>
              <p className="wizard-note">הטקסט יוקלד אוטומטית בשדה הפעיל.</p>
            </div>
            <button className="btn-wizard-next" onClick={() => setWizardStep(2)}>
              המשך
            </button>
          </div>
        )}

        {wizardStep === 2 && (
          <div className="wizard-step">
            <h2 className="wizard-step-title">בחר מצב תמלול</h2>

            <div
              className={`wizard-card ${wizardChoice === "api" ? "selected" : ""}`}
              onClick={() => setWizardChoice("api")}
            >
              <div className="wizard-card-header">
                <strong>☁️ API — מהיר ומדויק</strong>
                <span className="wizard-card-badge">מומלץ</span>
              </div>
              <p className="wizard-card-desc">תמלול בענן דרך Deepgram — מהיר, מדויק, דורש מפתח API (חינם לניסיון)</p>
              {wizardChoice === "api" && (
                <div className="wizard-guide">
                  <ol>
                    <li>היכנס ל-<strong>deepgram.com</strong></li>
                    <li>צור חשבון חינם ($200 קרדיט)</li>
                    <li>לחץ <strong>Create API Key</strong></li>
                    <li>הדבק את המפתח כאן:</li>
                  </ol>
                  <div className="api-key-row">
                    <input
                      type="password"
                      className="api-key-input"
                      value={wizardApiKey}
                      onChange={(e) => { setWizardApiKey(e.target.value); setWizardKeyValid(null); }}
                      placeholder="API key..."
                    />
                    <button
                      className={`btn-test ${wizardKeyValid === true ? "valid" : wizardKeyValid === false ? "invalid" : ""}`}
                      onClick={handleWizardTestKey}
                      disabled={wizardKeyTesting || !wizardApiKey}
                    >
                      {wizardKeyTesting ? "..." : wizardKeyValid === true ? "✓" : wizardKeyValid === false ? "✗" : "בדוק"}
                    </button>
                  </div>
                  {wizardKeyValid === true && <p className="settings-note success-note">המפתח תקין!</p>}
                  {wizardKeyValid === false && <p className="settings-note error-note">המפתח לא תקין</p>}
                </div>
              )}
            </div>

            <div
              className={`wizard-card ${wizardChoice === "local" ? "selected" : ""}`}
              onClick={() => setWizardChoice("local")}
            >
              <div className="wizard-card-header">
                <strong>💻 מקומי — פרטיות מלאה</strong>
                <span className="wizard-card-badge">ללא אינטרנט</span>
              </div>
              <p className="wizard-card-desc">תמלול על המחשב שלך. ללא שליחת נתונים. דורש הורדת מודל (75MB-1.5GB)</p>
              {wizardChoice === "local" && (
                <div className="wizard-guide">
                  <p className="settings-note">בשלב הבא תוכל להוריד מודל — <strong>small</strong> מומלץ לרוב המחשבים.</p>
                </div>
              )}
            </div>

            <div className="wizard-nav">
              <button className="btn-wizard-back" onClick={() => setWizardStep(1)}>חזור</button>
              <button
                className="btn-wizard-next"
                onClick={() => setWizardStep(3)}
                disabled={!wizardChoice}
              >
                {wizardChoice ? "המשך" : "בחר מצב"}
              </button>
            </div>
          </div>
        )}

        {wizardStep === 3 && (
          <div className="wizard-step">
            <div className="wizard-icon">✅</div>
            <h2 className="wizard-step-title">הכל מוכן!</h2>
            <div className="wizard-content">
              {wizardChoice === "api" && wizardApiKey ? (
                <p className="wizard-success">Deepgram מוגדר — תמלול מהיר ומדויק</p>
              ) : wizardChoice === "local" ? (
                <p className="wizard-note">מצב מקומי — הורד מודל בהגדרות כדי להתחיל</p>
              ) : (
                <p className="wizard-note">ניתן להגדיר מפתח API או להוריד מודל בהגדרות</p>
              )}
              <div className="wizard-highlight">
                <span>נסה עכשיו: לחץ</span>
                <span className="wizard-key">Alt + D</span>
                <span>ודבר בעברית</span>
              </div>
            </div>
            <div className="wizard-final-actions">
              <button className="btn-wizard-next" onClick={completeOnboarding}>
                התחל להקליט
              </button>
            </div>
          </div>
        )}
      </main>
    );
  }

  // ---- SETTINGS VIEW ----
  if (view === "settings") {
    return (
      <main className="container" dir="rtl">
        <div className="settings-header">
          <h2>הגדרות</h2>
          <button className="btn-back" onClick={() => setView("main")}>
            חזור
          </button>
        </div>

        {/* Transcription Engine */}
        <div className="settings-section">
          <h3>מנוע תמלול</h3>
          <div className="settings-row">
            {([
              ["api", "API (ענן)"],
              ["local", "מקומי (offline)"],
              ["auto_fallback", "אוטומטי"],
            ] as [TranscriptionMode, string][]).map(([mode, label]) => (
              <button
                key={mode}
                className={`btn-option ${transcriptionMode === mode ? "active" : ""}`}
                onClick={() => {
                  setTranscriptionMode(mode);
                  persistSettings({ transcription_mode: mode });
                }}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="settings-note">
            {transcriptionMode === "api" && "API: מהיר ומדויק, דורש מפתח Deepgram/OpenAI וחיבור אינטרנט"}
            {transcriptionMode === "local" && "מקומי: פרטיות מלאה, ללא אינטרנט, דורש מודל מותקן"}
            {transcriptionMode === "auto_fallback" && "אוטומטי: API עם גיבוי מקומי כשאין חיבור או מפתח"}
          </p>
        </div>

        {/* API Provider + Key */}
        {transcriptionMode !== "local" && (
          <div className="settings-section">
            <h3>ספק API</h3>
            <div className="settings-row" style={{ marginBottom: "0.75rem" }}>
              {([
                ["deepgram", "Deepgram (מומלץ)"],
                ["open_ai", "OpenAI Whisper"],
              ] as [ApiProvider, string][]).map(([prov, label]) => (
                <button
                  key={prov}
                  className={`btn-option ${apiProvider === prov ? "active" : ""}`}
                  onClick={() => {
                    setApiProvider(prov);
                    setApiKeyValid(null);
                    persistSettings({ api_provider: prov });
                  }}
                >
                  {label}
                </button>
              ))}
            </div>
            <p className="settings-note" style={{ marginBottom: "0.75rem" }}>
              {apiProvider === "deepgram" ? "Deepgram Nova-3: מהיר במיוחד, תמיכה בעברית ($0.0043/דקה)" : "OpenAI Whisper: דיוק גבוה, $0.006/דקה"}
            </p>

            <h3>מפתח {apiProvider === "deepgram" ? "Deepgram" : "OpenAI"}</h3>
            <div className="api-key-row">
              <input
                type="password"
                className="api-key-input"
                value={apiProvider === "open_ai" ? openaiKey : deepgramKey}
                onChange={(e) => {
                  if (apiProvider === "open_ai") setOpenaiKey(e.target.value);
                  else setDeepgramKey(e.target.value);
                  setApiKeyValid(null);
                }}
                onBlur={() => persistSettings(
                  apiProvider === "open_ai"
                    ? { openai_api_key: openaiKey || null }
                    : { deepgram_api_key: deepgramKey || null }
                )}
                placeholder={apiProvider === "open_ai" ? "sk-..." : "API key..."}
              />
              <button
                className={`btn-test ${apiKeyValid === true ? "valid" : apiKeyValid === false ? "invalid" : ""}`}
                onClick={handleTestApiKey}
                disabled={testingApiKey || !activeApiKey}
              >
                {testingApiKey ? "..." : apiKeyValid === true ? "✓" : apiKeyValid === false ? "✗" : "בדוק"}
              </button>
            </div>
            {apiKeyValid === false && (
              <p className="settings-note error-note">המפתח לא תקין — בדוק שהמפתח נכון</p>
            )}
            {apiKeyValid === true && (
              <p className="settings-note success-note">המפתח תקין</p>
            )}
            <p className="settings-note">
              {apiProvider === "deepgram" ? "קבל מפתח: deepgram.com" : "קבל מפתח: platform.openai.com"}
              {activeApiKey && " | מפתח מוגדר"}
            </p>
          </div>
        )}

        {/* Language */}
        <div className="settings-section">
          <h3>שפת תמלול</h3>
          <div className="settings-row">
            {(["he", "en", "auto"] as Language[]).map((lang) => (
              <button
                key={lang}
                className={`btn-option ${language === lang ? "active" : ""}`}
                onClick={() => {
                  setLanguage(lang);
                  persistSettings({ language: lang });
                }}
              >
                {langLabels[lang]}
              </button>
            ))}
          </div>
        </div>

        {/* VAD */}
        <div className="settings-section">
          <h3>עצירה אוטומטית (VAD)</h3>
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
              עצור הקלטה אוטומטית כשמפסיקים לדבר (2.5 שניות שקט)
            </span>
          </label>
        </div>

        {/* Microphone */}
        <div className="settings-section">
          <h3>מיקרופונים ({devices.length})</h3>
          {devices.length > 0 ? (
            <ul className="device-list">
              {devices.map((d, i) => <li key={i}>{d}</li>)}
            </ul>
          ) : (
            <p className="settings-note">לא נמצאו מיקרופונים</p>
          )}
          <p className="settings-note">המערכת משתמשת במיקרופון ברירת המחדל של Windows</p>
        </div>

        {/* Model Manager */}
        <div className="settings-section">
          <h3>ניהול מודלים מקומיים</h3>
          {transcriptionMode === "api" && (
            <p className="settings-note">מודלים מקומיים נדרשים רק למצב מקומי / אוטומטי</p>
          )}
          {activeModel && (
            <p className="settings-note active-note">מודל פעיל: <strong>{activeModel}</strong></p>
          )}
          <div className="model-cards">
            {models.map((m) => {
              const isActive = activeModel === m.name;
              const isDownloading = downloadingModel === m.name;
              return (
                <div key={m.name} className={`model-card ${isActive ? "active" : ""} ${m.downloaded ? "downloaded" : ""}`}>
                  <div className="model-card-header">
                    <span className="model-name">
                      {m.name}
                      {isActive && <span className="active-dot" />}
                    </span>
                    <span className="model-size">{m.size_label}</span>
                  </div>
                  <p className="model-desc">{m.description}</p>
                  <div className="model-card-actions">
                    {m.downloaded ? (
                      <>
                        <span className="tag-downloaded">מותקן</span>
                        {!isActive && (
                          <button onClick={() => handleActivateModel(m.name)} className="btn-activate" disabled={status === "loading-model"}>
                            הפעל
                          </button>
                        )}
                        <button onClick={() => handleDeleteModel(m.name)} className="btn-delete" disabled={isActive && status === "recording"}>
                          מחק
                        </button>
                      </>
                    ) : isDownloading ? (
                      <div className="mini-progress">
                        <div className="progress-bar">
                          <div className="progress-fill" style={{ width: `${downloadProgress}%` }} />
                        </div>
                        <span className="progress-label">{downloadProgress}%</span>
                      </div>
                    ) : (
                      <button onClick={() => handleDownloadModel(m.name)} className="btn-primary btn-small" disabled={status === "downloading"}>
                        הורד
                      </button>
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
          <p className="about-app-name">הכתבה בעברית v2.0.0</p>
          <p className="about-brand">BinTech AI — הנרי שטאובר</p>
          <p className="settings-note" style={{ marginTop: "0.5rem" }}>קוד פתוח — נבנה עם Tauri, Whisper, ו-❤️</p>
          <div className="about-links">
            <p className="settings-note">📧 henrystauber22@gmail.com</p>
            <p className="settings-note">🔗 taplink.cc/henry.ai</p>
            <p className="settings-note">🎥 youtube.com/@AIWithHenry</p>
          </div>
          <p className="settings-note" style={{ marginTop: "0.75rem" }}>Alt+D: הקלט/עצור מכל מקום | מגבלת הקלטה: {maxRecordingSecs}s</p>
        </div>

        {error && (
          <p className="error" onClick={() => setError("")}>❌ {error}</p>
        )}
      </main>
    );
  }

  // ---- MAIN VIEW ----
  const dismissCloseTip = async () => {
    setShowCloseTip(false);
    await persistSettings({ close_notification_shown: true });
  };

  return (
    <main className="container" dir="rtl">
      {showCloseTip && (
        <div className="close-tip-banner">
          <span>💡 האפליקציה ממשיכה לפעול ברקע. Alt+D עובד גם כשהחלון סגור.</span>
          <button className="btn-close-tip" onClick={dismissCloseTip}>הבנתי</button>
        </div>
      )}
      <div className="main-header">
        <h1>הכתבה בעברית</h1>
        <button className="btn-settings" onClick={() => setView("settings")} title="הגדרות">
          ⚙
        </button>
      </div>

      {/* No model — first-time setup (skip if API is configured) */}
      {models.length > 0 && downloadedCount === 0 && !apiKeyConfigured && status !== "downloading" && (
        <div className="setup-section">
          <p>יש להוריד מודל תמלול או להגדיר מפתח API בהגדרות</p>
          <div className="model-cards">
            {models.filter(m => ["small", "medium"].includes(m.name)).map((m) => (
              <div key={m.name} className="model-card">
                <div className="model-card-header">
                  <span className="model-name">{m.name}</span>
                  <span className="model-size">{m.size_label}</span>
                </div>
                <p className="model-desc">{m.description}</p>
                <button onClick={() => handleDownloadModel(m.name)} className="btn-primary btn-small">הורד</button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Downloading */}
      {status === "downloading" && downloadingModel && (
        <div className="download-section">
          <p>מוריד מודל {downloadingModel}... {downloadProgress}%</p>
          <div className="progress-bar">
            <div className="progress-fill" style={{ width: `${downloadProgress}%` }} />
          </div>
        </div>
      )}

      {/* Status */}
      <div className="status-section">
        <div className={`status-indicator ${status} ${showTimeWarning ? "warning" : ""}`}>
          {statusLabels[status]}
        </div>
        {showTimeWarning && (
          <p className="time-warning">נותרו {Math.ceil(timeRemaining)} שניות</p>
        )}
      </div>

      {/* Record button */}
      <div className="controls">
        <button
          onClick={handleToggleRecording}
          className={`btn-record ${status === "recording" ? "recording" : ""}`}
          disabled={status === "transcribing" || status === "downloading" || status === "loading-model" || !canRecord}
        >
          {status === "recording" ? "⏹ עצור והכתב" : "🎤 הקלט"}
        </button>
      </div>

      {/* Recording progress bar */}
      {status === "recording" && (
        <div className="recording-progress">
          <div className="progress-bar">
            <div
              className={`progress-fill ${showTimeWarning ? "warning" : ""}`}
              style={{ width: `${(recordingTime / maxRecordingSecs) * 100}%` }}
            />
          </div>
        </div>
      )}

      {/* Transcript */}
      {transcript && (
        <div className="transcript-section">
          <h3>תמלול אחרון:</h3>
          <p className="transcript-text">{transcript}</p>
          <div className="transcript-actions">
            <button onClick={() => invoke("inject_text", { text: transcript })} className="btn-secondary">
              ⌨️ הקלד
            </button>
            <button onClick={() => navigator.clipboard.writeText(transcript)} className="btn-secondary">
              📋 העתק
            </button>
          </div>
        </div>
      )}

      {error && (
        <p className="error" onClick={() => setError("")}>❌ {error}</p>
      )}

      {history.length > 0 && (
        <div className="history-section">
          <h3>היסטוריה:</h3>
          {history.map((h) => (
            <p key={h.id} className="history-item" onClick={() => navigator.clipboard.writeText(h.text)} title="לחץ להעתקה">
              {h.text}
            </p>
          ))}
        </div>
      )}

      <div className="footer">
        <p>
          Alt+D: הקלט/עצור מכל מקום | {langLabels[language]} | {vadEnabled ? "עצירה אוטומטית" : "עצירה ידנית"}
        </p>
        <p className="footer-brand">BinTech AI — הנרי שטאובר</p>
      </div>
    </main>
  );
}

export default App;

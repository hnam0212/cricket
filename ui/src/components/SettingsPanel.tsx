import type { Settings } from "../api";
import { defaultSettings, strings } from "../strings";

interface SettingsPanelProps {
  settings: Settings;
  minimizeToTray: boolean;
  onChange: (settings: Settings) => void;
  onMinimizeToTray: (value: boolean) => void;
}

type NumberKey = "trigger_delay_ms" | "resume_cooldown_ms" | "fade_out_ms" | "fade_in_ms";

const durations: { key: NumberKey; label: string; help: string }[] = [
  { key: "trigger_delay_ms", label: strings.triggerDelay, help: strings.triggerDelayHelp },
  { key: "resume_cooldown_ms", label: strings.resumeCooldown, help: strings.resumeCooldownHelp },
  { key: "fade_out_ms", label: strings.fadeOut, help: strings.fadeOutHelp },
  { key: "fade_in_ms", label: strings.fadeIn, help: strings.fadeInHelp },
];

export function SettingsPanel({
  settings,
  minimizeToTray,
  onChange,
  onMinimizeToTray,
}: SettingsPanelProps) {
  const setNumber = (key: NumberKey | "sound_threshold", text: string) => {
    const value = Number(text);
    // Ignore a half-typed or empty field instead of saving a zero.
    if (text.trim() === "" || !Number.isFinite(value) || value < 0) return;
    onChange({ ...settings, [key]: key === "sound_threshold" ? value : Math.round(value) });
  };

  return (
    <details className="panel">
      <summary>{strings.settingsHeading}</summary>

      {durations.map(({ key, label, help }) => (
        <label className="field" key={key}>
          <span className="field-label">{label}</span>
          <span className="field-input">
            <input
              type="number"
              min={0}
              max={60000}
              step={100}
              value={settings[key]}
              onChange={(event) => setNumber(key, event.target.value)}
            />
            {strings.milliseconds}
          </span>
          <span className="hint">{help}</span>
        </label>
      ))}

      <label className="field">
        <span className="field-label">{strings.threshold}</span>
        <span className="field-input">
          <input
            type="number"
            min={0}
            max={1}
            step={0.01}
            value={settings.sound_threshold}
            onChange={(event) => setNumber("sound_threshold", event.target.value)}
          />
        </span>
        <span className="hint">{strings.thresholdHelp}</span>
      </label>

      <label className="check">
        <input
          type="checkbox"
          checked={settings.mic_counts_as_activity}
          onChange={(event) =>
            onChange({ ...settings, mic_counts_as_activity: event.target.checked })
          }
        />
        {strings.micCounts}
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={settings.ignore_system_sounds}
          onChange={(event) =>
            onChange({ ...settings, ignore_system_sounds: event.target.checked })
          }
        />
        {strings.ignoreSystemSounds}
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={minimizeToTray}
          onChange={(event) => onMinimizeToTray(event.target.checked)}
        />
        {strings.minimizeToTray}
      </label>

      <button type="button" onClick={() => onChange({ ...defaultSettings })}>
        {strings.resetSettings}
      </button>
    </details>
  );
}

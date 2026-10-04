interface SwitchProps {
  checked: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}

/** The main on/off switch. A checkbox underneath, so it works with the keyboard. */
export function Switch({ checked, label, onChange }: SwitchProps) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        role="switch"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="switch-track" aria-hidden="true" />
      <span className="switch-label">{label}</span>
    </label>
  );
}

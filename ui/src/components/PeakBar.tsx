interface PeakBarProps {
  peak: number;
  makingSound: boolean;
}

/** A level meter for one app. */
export function PeakBar({ peak, makingSound }: PeakBarProps) {
  const percent = Math.round(Math.min(Math.max(peak, 0), 1) * 100);
  return (
    <span className={makingSound ? "peak peak-on" : "peak"} aria-hidden="true">
      <span className="peak-fill" style={{ width: `${percent}%` }} />
    </span>
  );
}

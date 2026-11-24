import { useEffect, useRef, useState } from "react";
import { SliderCommit } from "../appearance-controller";
import "./TranslucencySlider.css";
export interface TranslucencySliderProps {
  id: string;
  label: string;
  value: number;
  /** The position to save: once per drag, where the slider comes to rest. */
  onChange: (value: number) => void;
  /** Every step of a drag, to paint without saving or re-rendering the page. */
  onPreview?: (value: number) => void;
  disabled?: boolean;
}
export function TranslucencySlider({ id, label, value, onChange, onPreview, disabled }: TranslucencySliderProps) {
  // The dragged position lives here until it is saved, so a step re-renders
  // this slider and nothing else.
  const [draft, setDraft] = useState<number | null>(null);
  const save = useRef(onChange);
  save.current = onChange;
  const [commit] = useState(() => new SliderCommit(next => { setDraft(null); save.current(next); }));
  useEffect(() => () => { commit.flush(); }, [commit]);
  const shown = draft ?? value;
  return <div className="translucency-slider">
    <div className="translucency-slider-heading">
      <label htmlFor={id}>{label}</label>
      <output htmlFor={id}>{shown}%</output>
    </div>
    <input id={id} type="range" min={0} max={100} step={1} value={shown} disabled={disabled}
      aria-valuetext={`${shown}% translucency${shown === 0 ? ", solid" : shown === 100 ? ", glass" : ""}`}
      onChange={event => {
        const next = event.currentTarget.valueAsNumber;
        setDraft(next);
        onPreview?.(next);
        commit.input(next);
      }}
      onPointerUp={() => { commit.flush(); }} onKeyUp={() => { commit.flush(); }}
      onBlur={() => { commit.flush(); }} />
    <div className="translucency-slider-endpoints" aria-hidden="true"><span>Solid</span><span>Glass</span></div>
  </div>;
}

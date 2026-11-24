import "./FormSelect.css";
import { AppSelect, type AppSelectOption } from "./AppSelect";

// Above this many rows a picker earns its filter field. Below it, the filter is
// one more thing between the human and four options.
const SEARCHABLE_ROWS = 8;


export interface FormSelectProps {
  label: string;
  value: string;
  options: readonly AppSelectOption[];
  onChange: (value: string) => void;
  name?: string;
  placeholder?: string;
  disabled?: boolean;
  /**
   * The surface's own field classes, appended to the shared `form-select`.
   * Each surface keeps the field rhythm its neighbours have; the picker inside
   * is the same control everywhere.
   */
  fieldClassName?: string;
  /** Scope the portalled menu when its form lives inside a modal overlay. */
  popoverClassName?: string;
}

/**
 * One picker in the app's own idiom rather than the platform's. The optional
 * hidden input is what keeps the FormData submit paths unchanged: the visible
 * control is ours, while the value the form reads is still a named field on the
 * form.
 *
 * This started life as CanvasSelect inside AgentCanvas.tsx, which was the one
 * surface in the app that had worked out the adapter — while thirty-four
 * platform <select>s stayed on the plan page and the two panels beside it,
 * a different height and a different focus ring from every other control there.
 */
export function FormSelect(props: FormSelectProps) {
  const className = props.fieldClassName === undefined
    ? "form-select" : `form-select ${props.fieldClassName}`;
  return <div className={className}>
    <span>{props.label}</span>
    <AppSelect label={props.label} value={props.value} options={props.options}
      onChange={props.onChange} disabled={props.disabled}
      popoverClassName={props.popoverClassName}
      searchable={props.options.length > SEARCHABLE_ROWS}
      {...(props.placeholder === undefined ? {} : { placeholder: props.placeholder })} />
    {props.name !== undefined
      && <input type="hidden" name={props.name} value={props.value} />}
  </div>;
}

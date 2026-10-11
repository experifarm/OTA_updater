// Tiny DOM helpers for the chapter widgets.

type Child = Node | string | number | null | undefined | false;

export interface Props {
  class?: string;
  text?: string;
  title?: string;
  style?: string;
  html?: never;
  on?: Partial<Record<keyof HTMLElementEventMap, (e: Event) => void>>;
  attrs?: Record<string, string | number | boolean>;
}

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props | null = null,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props) {
    if (props.class) el.className = props.class;
    if (props.text !== undefined) el.textContent = props.text;
    if (props.title) el.title = props.title;
    if (props.style) el.setAttribute('style', props.style);
    for (const [k, v] of Object.entries(props.attrs ?? {})) {
      if (v === false) continue;
      el.setAttribute(k, v === true ? '' : String(v));
    }
    for (const [ev, fn] of Object.entries(props.on ?? {})) if (fn) el.addEventListener(ev, fn);
  }
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.append(typeof c === 'number' ? String(c) : c);
  }
  return el;
}

export const hex2 = (b: number) => b.toString(16).padStart(2, '0');
export const bits8 = (b: number) => b.toString(2).padStart(8, '0');

/** A printable rendering of one byte (control characters as symbols). */
export function glyph(b: number): string {
  if (b === 10) return '↵';
  if (b === 32) return '␣';
  if (b >= 33 && b < 127) return String.fromCharCode(b);
  return '·';
}

/** A labelled range slider. `fmt` renders the value next to it. */
export function slider(
  label: string,
  value: number,
  min: number,
  max: number,
  step: number,
  onInput: (v: number) => void,
  fmt: (v: number) => string = String,
): HTMLLabelElement {
  const out = h('output', { text: fmt(value) });
  const input = h('input', {
    attrs: { type: 'range', min, max, step, value },
    on: {
      input: (e) => {
        const v = Number((e.target as HTMLInputElement).value);
        out.textContent = fmt(v);
        onInput(v);
      },
    },
  });
  return h('label', { class: 'ctl' }, h('span', { text: label }), input, out);
}

export function button(label: string, onClick: () => void, cls = ''): HTMLButtonElement {
  return h('button', { class: cls, text: label, attrs: { type: 'button' }, on: { click: onClick } });
}

/** Debounce for text inputs that re-run the engine. */
export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number): (...a: A) => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (...a: A) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...a), ms);
  };
}

/** Golden-angle hues shared with the sandbox renderer. */
export function hue(g: number): string {
  return `hsl(${((g * 137.508) % 360).toFixed(1)} 72% 60%)`;
}

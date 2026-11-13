// What a browser is asked to do for some video effects settings (`videofxmodel.ts`): `videoLook`
// turns them into a CSS `filter` for the `<video>` (the browser's own functions where they are the
// same sum, SVG colour filters for the rest) and a few layers over the picture. Nothing is drawn by
// us, so fullscreen, picture in picture and AirPlay keep working, and the default is nothing at
// all. Each effect follows its shader in `crates/corrent-core/src/video_fx/shaders/`.
import { CATALOGUE, isOn, value, type VideoFx } from './videofxmodel';

export interface Overlay {
  kind: 'scanlines' | 'vignette' | 'grain';
  /** CSS declarations for a layer that covers the picture. */
  style: string;
}

export interface VideoLook {
  /** The `<video>`'s CSS `filter`, empty for none. */
  filter: string;
  /** `<filter>` elements (SVG markup made of numbers only) for the `url(#…)` parts of `filter`. */
  defs: string;
  /** Layers over the picture, bottom first. */
  overlays: Overlay[];
}

export interface View {
  /** The height of the picture as shown, in CSS pixels: sizes follow it as they follow the
   *  output's height on the desktop. */
  height?: number;
}

const LUMA = [0.2126, 0.7152, 0.0722];
/** A number as CSS and SVG take it: short, never an exponent. */
const n = (v: number) => String(Math.round(v * 1e4) / 1e4 + 0);
const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
const smoothstep = (a: number, b: number, x: number) => {
  const t = clamp01((x - a) / (b - a));
  return t * t * (3 - 2 * t);
};
const mixn = (a: number, b: number, t: number) => a + (b - a) * t;

type Rows = [number[], number[], number[]];

/** The primitives of one `<filter>`, each reading the one before. All work on the encoded (sRGB)
 *  values like the shaders do, except where `linear` says otherwise. */
class Graph {
  out: string[] = [];
  last = 'SourceGraphic';

  private add(tag: string, attrs: string, body = ''): string {
    const r = `r${this.out.length}`;
    this.out.push(`<${tag} ${attrs} result="${r}"${body ? `>${body}</${tag}>` : '/>'}`);
    return (this.last = r);
  }
  /** Rows of r, g, b and an optional constant. */
  matrix(m: Rows, linear = false): string {
    const values = m.map((r) => `${n(r[0])} ${n(r[1])} ${n(r[2])} 0 ${n(r[3] ?? 0)}`).join(' ');
    return this.add('feColorMatrix', `in="${this.last}" type="matrix" values="${values} 0 0 0 1 0"${linear ? ' color-interpolation-filters="linearRGB"' : ''}`);
  }
  /** A curve per channel (`feFunc` attributes). */
  transfer(r: string, g = r, b = r): string {
    return this.add('feComponentTransfer', `in="${this.last}"`, `<feFuncR ${r}/><feFuncG ${g}/><feFuncB ${b}/>`);
  }
  /** `k` × a × b, channel by channel. */
  times(a: string, b: string, k: number): string {
    return this.add('feComposite', `in="${a}" in2="${b}" operator="arithmetic" k1="${n(k)}" k2="0" k3="0" k4="0"`);
  }
  /** `ka` × a + `kb` × b + `k`. Alpha goes the same way: the three must not add up to less than 1. */
  sum(a: string, b: string, ka: number, kb: number, k = 0): string {
    return this.add('feComposite', `in="${a}" in2="${b}" operator="arithmetic" k1="0" k2="${n(ka)}" k3="${n(kb)}" k4="${n(k)}"`);
  }
  /** `t` of a, the rest of b. */
  mix(a: string, b: string, t: number): string {
    return this.sum(a, b, t, 1 - t);
  }
  /** The brightness of every pixel, as a grey picture. */
  luma(): string {
    return this.matrix([LUMA, LUMA, LUMA]);
  }
}

/** A curve from 0..1 to 0..1 as a table the browser interpolates. */
function table(f: (x: number) => number, points = 33): string {
  const values = Array.from({ length: points }, (_, i) => n(clamp01(f(i / (points - 1)))));
  return `type="table" tableValues="${values.join(' ')}"`;
}

/** Machado, Oliveira and Fernandes (2009) at full severity, on linear light: protanomaly,
 *  deuteranomaly, tritanomaly. The same numbers as the desktop's `colour_blind.glsl`. */
const SEES: Rows[] = [
  [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
  ],
  [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.01182, 0.04294, 0.968881],
  ],
  [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.3039],
  ],
];
const IDENTITY: Rows = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
];
/** mpv's picture controls as one matrix: the colour of a pixel (its Cb and Cr, BT.709) turned by
 *  `turn` × 180° and scaled by `saturation`, all of it times `gain`, plus `offset`. */
function adjustMatrix(saturation: number, turn: number, gain: number, offset: number): Rows {
  const [kr, kg, kb] = LUMA;
  const [cos, sin] = [Math.cos(turn * Math.PI), Math.sin(turn * Math.PI)];
  const column = (rgb: number[]) => {
    const y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
    const [u, v] = [(rgb[2] - y) / (2 * (1 - kb)), (rgb[0] - y) / (2 * (1 - kr))];
    const [r, b] = [y + 2 * (1 - kr) * saturation * (-sin * u + cos * v), y + 2 * (1 - kb) * saturation * (cos * u + sin * v)];
    return [r, (y - kr * r - kb * b) / kg, b].map((x) => x * gain);
  };
  const columns = IDENTITY.map(column);
  return [0, 1, 2].map((i) => [...columns.map((c) => c[i]), offset]) as Rows;
}
const kindOf = (v: number) => Math.max(0, Math.min(2, Math.round(v)));

type Step = string | ((g: Graph) => void);

/** One effect as steps of the filter. Each follows its shader in `video_fx/shaders/`. */
function effectSteps(fx: VideoFx, id: string, px: number, steps: Step[], overlays: Overlay[]): void {
  const v = (key: string) => value(fx, id, key);
  switch (id) {
    case 'soften': {
      // The shader mixes in a 3×3 blur of taps 1.5 px apart: a Gaussian of the same spread.
      const sigma = 1.5 * px * Math.sqrt(v('amount') / 2);
      if (sigma > 0) steps.push(`blur(${n(Math.round(sigma * 100) / 100)}px)`);
      break;
    }
    case 'levels': {
      const black = v('black');
      const span = Math.max(v('white') - black, 0.01);
      const mid = v('midtones');
      if (black === 0 && span === 1 && mid === 1) break;
      // With midtones, a table of every 8-bit value: two curves in a row would lose the darks.
      steps.push((g) =>
        g.transfer(mid === 1 ? `type="linear" slope="${n(1 / span)}" intercept="${n(-black / span)}"` : table((x) => clamp01((x - black) / span) ** (1 / mid), 256)),
      );
      break;
    }
    case 'temperature': {
      const [w, t] = [v('warmth'), v('tint')];
      if (w === 0 && t === 0) break;
      const gain = [1 + 0.22 * w + 0.08 * t, 1 - 0.16 * t, 1 - 0.22 * w + 0.08 * t];
      // The shader keeps every pixel's brightness; a matrix can keep that of the greys.
      const keep = gain[0] * LUMA[0] + gain[1] * LUMA[1] + gain[2] * LUMA[2];
      steps.push((g) => g.matrix([[gain[0] / keep, 0, 0], [0, gain[1] / keep, 0], [0, 0, gain[2] / keep]]));
      break;
    }
    case 'tone': {
      const [s, h] = [v('shadows'), v('highlights')];
      if (s === 0 && h === 0) break;
      // Every pixel times a factor of its brightness: the factor, scaled into 0..1, as a picture.
      const factor = (l: number) => 1 + (s * 0.35 * (1 - l) ** 3 + h * 0.35 * l ** 3) / Math.max(l, 0.25);
      const top = Math.max(1, factor(0), factor(0.25), factor(1));
      steps.push((g) => {
        const src = g.last;
        g.luma();
        const f = g.transfer(table((l) => factor(l) / top));
        g.times(src, f, top);
      });
      break;
    }
    case 'cinema': {
      const amount = v('amount');
      if (amount === 0) break;
      steps.push((g) => {
        const src = g.last;
        g.luma();
        const top = 1.12;
        const tint = g.transfer(
          table((l) => mixn(0.84, 1.12, smoothstep(0, 1, l)) / top),
          table(() => 1 / top, 2),
          table((l) => mixn(1.12, 0.84, smoothstep(0, 1, l)) / top),
        );
        g.times(src, tint, top);
        const graded = g.transfer(table((x) => mixn(x, x * x * (3 - 2 * x), 0.5)));
        if (amount < 1) g.mix(graded, src, amount);
      });
      break;
    }
    case 'cross_process': {
      const amount = v('amount');
      if (amount === 0) break;
      const curve = (x: number) => x * x * (3 - 2 * x);
      steps.push((g) =>
        g.transfer(
          table((x) => mixn(x, curve(x), amount)),
          table((x) => mixn(x, mixn(x, curve(x), 0.6) * 1.05, amount)),
          table((x) => mixn(x, mixn(x, 0.25 + 0.5 * x, 0.7), amount)),
        ),
      );
      break;
    }
    case 'mono':
      if (v('amount') > 0) steps.push(`grayscale(${n(v('amount'))})`);
      break;
    case 'sepia':
      if (v('amount') > 0) steps.push(`sepia(${n(v('amount'))})`);
      break;
    case 'daltonize': {
      const strength = v('strength');
      if (strength === 0) break;
      const kind = kindOf(v('kind'));
      const sees = SEES[kind];
      // What this viewer doesn't see, moved to the channels they do.
      const move: Rows =
        kind < 2
          ? [
              [0, 0, 0],
              [0.7, 1, 0],
              [0.7, 0, 1],
            ]
          : [
              [1, 0, 0.7],
              [0, 1, 0.7],
              [0, 0, 0],
            ];
      // c + strength × move × (c − seen): the part of c and the part of what is seen as two
      // pictures in 0..1 (the second one upside down, so nothing is negative), then their sum.
      const top = 1 + 1.7 * strength;
      steps.push((g) => {
        const src = g.last;
        g.matrix(sees, true);
        const minus = g.matrix(move.map((r) => [...r.map((x) => (-strength * x) / top), 1]) as Rows);
        g.last = src;
        const plus = g.matrix(IDENTITY.map((r, i) => r.map((x, j) => (x + strength * move[i][j]) / top)) as Rows);
        g.sum(plus, minus, top, top, -top);
      });
      break;
    }
    case 'colour_blind': {
      const severity = v('severity');
      if (severity === 0) break;
      steps.push((g) => {
        const src = g.last;
        const seen = g.matrix(SEES[kindOf(v('kind'))], true);
        if (severity < 1) g.mix(seen, src, severity);
      });
      break;
    }
    case 'posterize': {
      // Rounding to `levels` values: steps half as wide at both ends.
      const last = v('levels') - 1;
      const values = Array.from({ length: 2 * last }, (_, i) => n(Math.floor((i + 1) / 2) / last));
      steps.push((g) => g.transfer(`type="discrete" tableValues="${values.join(' ')}"`));
      break;
    }
    case 'invert':
      steps.push('invert(1)');
      break;
    case 'crt': {
      const lines = v('scanlines');
      if (lines === 0) break;
      steps.push(`brightness(${n(1 + 0.35 * lines)})`);
      const period = 3 * Math.max(px, 1);
      const stops = Array.from({ length: 9 }, (_, i) => `rgba(0,0,0,${n(lines * 0.7 * (0.5 - 0.5 * Math.cos((2 * Math.PI * i) / 8)))}) ${n(i * 12.5)}%`);
      overlays.push({ kind: 'scanlines', style: `background:linear-gradient(to bottom,${stops.join(',')}) 0 0/100% ${n(period)}px` });
      break;
    }
    case 'vignette': {
      const [strength, radius] = [v('strength'), v('radius')];
      if (strength === 0) break;
      // 0 in the middle, 1 in the corners, as the shader measures it.
      const [from, to] = [Math.max(0, radius - 0.4), Math.min(1, radius + 0.3)];
      const stops = Array.from({ length: 9 }, (_, i) => {
        const r = mixn(from, to, i / 8);
        return `rgba(0,0,0,${n(strength * smoothstep(radius - 0.4, radius + 0.3, r))}) ${n(r * 100)}%`;
      });
      overlays.push({ kind: 'vignette', style: `background:radial-gradient(ellipse farthest-corner at 50% 50%,${stops.join(',')})` });
      break;
    }
    case 'grain': {
      const amount = v('amount');
      if (amount === 0) break;
      const size = GRAIN_TILE * v('size') * px;
      overlays.push({ kind: 'grain', style: `opacity:${n(Math.min(1, amount * GRAIN_GAIN))};background-size:${n(size)}px ${n(size)}px` });
      break;
    }
  }
}

/** The side of the grain layer's noise picture in its own pixels (one per grain at size 1). */
export const GRAIN_TILE = 256;
/** The layer's opacity for an amount of 1: measured over greys, this spreads the picture's values
 *  about as far as the shader's noise does. */
const GRAIN_GAIN = 1.2;
/** The noise picture: white and black dots, the more opaque the further the noise is from grey,
 *  the black ones at 40 % (the shader has less grain in the lights). */
export const GRAIN_IMAGE =
  'url("data:image/svg+xml,' +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${GRAIN_TILE}" height="${GRAIN_TILE}">` +
      '<filter id="n" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB">' +
      '<feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" seed="7" stitchTiles="stitch"/>' +
      '<feColorMatrix type="matrix" values="1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 0 0 0 0"/>' +
      '<feComponentTransfer><feFuncR type="discrete" tableValues="0 1"/><feFuncG type="discrete" tableValues="0 1"/><feFuncB type="discrete" tableValues="0 1"/>' +
      '<feFuncA type="table" tableValues="0.4 0.4 0 1 1"/></feComponentTransfer></filter>' +
      '<rect width="100%" height="100%" filter="url(#n)"/></svg>',
  ) +
  '")';

/** FNV-1a, for a filter's id: a new text gets a new id, so every browser draws it anew. */
function hash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193);
  return (h >>> 0).toString(36);
}

const NONE: VideoLook = { filter: '', defs: '', overlays: [] };

/** What to do to the `<video>` for `fx`. Off, or nothing the browser can draw: nothing at all. */
export function videoLook(fx: VideoFx, view: View = {}): VideoLook {
  if (!fx.enabled) return NONE;
  // The desktop's PX: 1 at 1080 lines.
  const px = Math.max((view.height && view.height > 0 ? view.height : 1080) / 1080, 0.5);
  const steps: Step[] = [];
  const overlays: Overlay[] = [];

  // The picture controls as mpv does them (measured against it): saturation scales the colour
  // around the brightness and hue turns it, contrast is a gain from black, brightness is added,
  // and the result is clamped once before gamma, a power.
  const a = fx.adjust;
  const [saturation, gain] = [1 + a.saturation / 100, 1 + a.contrast / 100];
  if (a.hue || a.brightness) {
    // CSS's hue-rotate() turns the other way and around another axis, and it has no offset.
    steps.push((g) => g.matrix(adjustMatrix(saturation, a.hue / 100, gain, a.brightness / 100)));
  } else {
    // The browser's own functions are these sums (its brightness() is a gain from black).
    if (a.saturation) steps.push(`saturate(${n(saturation)})`);
    if (a.contrast) steps.push(`brightness(${n(gain)})`);
  }
  if (a.gamma) steps.push((g) => g.transfer(`type="gamma" amplitude="1" exponent="${n(8 ** (-a.gamma / 100))}" offset="0"`));

  for (const s of CATALOGUE) if (s.web && isOn(fx, s.id)) effectSteps(fx, s.id, px, steps, overlays);
  if (!steps.length && !overlays.length) return NONE;

  // Neighbouring SVG steps share one <filter>.
  const parts: string[] = [];
  const defs: string[] = [];
  let graph: Graph | null = null;
  const close = () => {
    if (!graph) return;
    const body = graph.out.join('');
    const id = `vfx-${defs.length}-${hash(body)}`;
    defs.push(`<filter id="${id}" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB">${body}</filter>`);
    parts.push(`url(#${id})`);
    graph = null;
  };
  for (const step of steps) {
    if (typeof step === 'string') {
      close();
      parts.push(step);
    } else step((graph ??= new Graph()));
  }
  close();
  return { filter: parts.join(' '), defs: defs.join(''), overlays };
}

/** Where the picture is inside its element (`object-fit: contain`), for the layers over it. */
export function pictureRect(box: { w: number; h: number }, video: { w: number; h: number }, contain: boolean): { x: number; y: number; w: number; h: number } {
  if (!contain || !(video.w > 0) || !(video.h > 0) || !(box.w > 0) || !(box.h > 0)) return { x: 0, y: 0, w: box.w, h: box.h };
  // The side that fills the element is exactly the element's.
  const wide = box.w * video.h <= box.h * video.w;
  const [w, h] = wide ? [box.w, (box.w * video.h) / video.w] : [(box.h * video.w) / video.h, box.h];
  return { x: (box.w - w) / 2, y: (box.h - h) / 2, w, h };
}

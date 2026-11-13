// The names of the video effects: the desktop's own words (src/app/media/picture.rs), so both
// clients read the same. Translate with `t(...)` where shown.
import { tk } from './i18n.svelte';
import type { Adjust, Group } from './videofxmodel';

/** An effect's name. */
export const LABEL: Record<string, string> = {
  denoise: tk('Reduce noise'),
  sharpen: tk('Sharpen'),
  clarity: tk('Clarity'),
  soften: tk('Soften'),
  levels: tk('Levels'),
  temperature: tk('Colour temperature'),
  tone: tk('Shadows and highlights'),
  vibrance: tk('Vibrance'),
  cinema: tk('Teal and orange'),
  cross_process: tk('Cross-process'),
  mono: tk('Black and white'),
  sepia: tk('Sepia'),
  daltonize: tk('Colour-blind correction'),
  colour_blind: tk('Colour-blind preview'),
  posterize: tk('Posterize'),
  invert: tk('Invert colours'),
  night_vision: tk('Night vision'),
  pixelate: tk('Pixelate'),
  chromatic: tk('Colour fringes'),
  bloom: tk('Glow'),
  outline: tk('Outlines'),
  crt: tk('Tube television'),
  halftone: tk('Dither'),
  vignette: tk('Vignette'),
  grain: tk('Film grain'),
  zebra: tk('Zebra stripes'),
};

/** What an effect does, for its hover text. */
export const HINT: Record<string, string> = {
  denoise: tk('Smooths grain and compression noise, keeping edges. Works on the source picture.'),
  sharpen: tk('Brings out fine detail without halos on strong edges.'),
  clarity: tk('More local contrast in the midtones: textures stand out.'),
  soften: tk('A slight blur that takes the edge off a harsh picture.'),
  levels: tk('Sets the black point, the white point and the midtones.'),
  temperature: tk('Warmer or cooler, and a green–magenta tint.'),
  tone: tk('Lifts or darkens the dark and the bright parts separately.'),
  vibrance: tk('Strengthens pale colours more than strong ones.'),
  cinema: tk('Cool shadows, warm lights and a little more contrast.'),
  cross_process: tk('The shifted colours of film developed in the wrong chemicals.'),
  mono: tk('Takes the colour out.'),
  sepia: tk('The brown tone of old photographs.'),
  daltonize: tk('Moves colours that are hard to tell apart to ones that are easier.'),
  colour_blind: tk('Shows the picture as someone with a colour vision deficiency sees it.'),
  posterize: tk('Fewer steps of each colour, like a poster.'),
  invert: tk('A negative of the picture.'),
  night_vision: tk('Green, amplified and noisy.'),
  pixelate: tk('Big square pixels.'),
  chromatic: tk('Red and blue drift apart towards the corners, like a cheap lens.'),
  bloom: tk('Bright parts glow into their surroundings.'),
  outline: tk('Darkens edges, like a drawing.'),
  crt: tk('Scanlines, a shadow mask and curved glass.'),
  halftone: tk('A few levels per colour in a fixed pattern, like old computer graphics.'),
  vignette: tk('Darker towards the corners.'),
  grain: tk('Fine moving noise, as on film.'),
  zebra: tk('Stripes over parts that are at full brightness.'),
};

const PARAM: Record<string, string> = {
  'levels.black': tk('Black'),
  'levels.white': tk('White'),
  'levels.midtones': tk('Midtones'),
  'temperature.warmth': tk('Warmth'),
  'temperature.tint': tk('Tint'),
  'tone.shadows': tk('Shadows'),
  'tone.highlights': tk('Highlights'),
  'denoise.edges': tk('Edges'),
  'clarity.radius': tk('Radius'),
  'vignette.radius': tk('Size'),
  'colour_blind.severity': tk('Severity'),
  'posterize.levels': tk('Levels'),
  'halftone.levels': tk('Levels'),
  'night_vision.gain': tk('Gain'),
  'night_vision.noise': tk('Noise'),
  'pixelate.block': tk('Block'),
  'bloom.threshold': tk('Threshold'),
  'zebra.threshold': tk('Threshold'),
  'bloom.intensity': tk('Intensity'),
  'crt.curvature': tk('Curve'),
  'crt.scanlines': tk('Scanlines'),
  'crt.mask': tk('Mask'),
  'halftone.cell': tk('Size'),
  'grain.size': tk('Size'),
  strength: tk('Strength'),
  amount: tk('Amount'),
};

/** A value's name, short enough for a slider. */
export const paramLabel = (id: string, key: string): string => PARAM[`${id}.${key}`] ?? PARAM[key] ?? '';

/** The choices of a colour vision `kind`, in its order. */
export const KINDS = [tk('Red-weak'), tk('Green-weak'), tk('Blue-weak')];

export const GROUP_LABEL: Record<Group, string> = {
  colour: tk('Colour'),
  detail: tk('Detail'),
  style: tk('Style'),
  accessibility: tk('Colour vision'),
  tools: tk('Tools'),
};

export const PRESET_LABEL: Record<string, string> = {
  vivid: tk('Vivid'),
  cinema: tk('Cinema'),
  warm: tk('Warm'),
  old_film: tk('Old film'),
  black_white: tk('Black and white'),
  retro_tv: tk('Retro TV'),
  comic: tk('Comic'),
  night: tk('Night'),
};

export const ADJUST_LABEL: Record<keyof Adjust, string> = {
  brightness: tk('Brightness'),
  contrast: tk('Contrast'),
  saturation: tk('Saturation'),
  gamma: tk('Gamma'),
  hue: tk('Hue'),
};

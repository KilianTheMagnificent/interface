/**
 * Shader backdrops (owner, 2026-11-15, "look at the current themes we have in
 * TN Code and port those over to Interface"). TN Code's Orbit and Glyph
 * themes draw their moving patterns with a fragment shader on one canvas
 * behind the window; a theme file can now carry one too, as `--theme-shader`.
 * theme-look.ts owns when it moves (the same stepper, rests and battery rules
 * as every other backdrop); this module only draws.
 *
 * A shader gets the canvas size (`u_resolution`), the seconds its pattern has
 * run (`u_time`) and, when the file embeds one, a picture (`u_image`,
 * `u_imageSize`). It can draw pixels on this canvas and nothing else.
 *
 * Cost, as TN Code keeps it: the canvas is drawn at a fraction of the
 * window's size and stretched (`image-rendering: pixelated`), never above
 * SHADER_MAX_PIXELS, at the file's own rate, on the low-power GPU.
 */

/** Keeps a 5K window from turning into a 15-megapixel canvas. */
export const SHADER_MAX_PIXELS = 4_200_000;
/** Canvas pixels per CSS pixel when the file does not say. */
export const SHADER_DEFAULT_SCALE = 0.5;
/** Frames a second when the file does not say; the backdrop's own rate caps it. */
export const SHADER_DEFAULT_RATE = 20;

/** A number a theme file wrote, held to a range; anything else is the fallback. */
export function shaderNumber(value: string, fallback: number, least: number, most: number): number {
  const text = value.trim();
  if (!/^[+-]?(?:\d+\.?\d*|\.\d+)$/u.test(text)) return fallback;
  return Math.min(Math.max(Number(text), least), most);
}

/** The canvas's size in its own pixels for a box of CSS pixels. */
export function shaderCanvasSize(cssWidth: number, cssHeight: number, scale: number): { width: number; height: number } {
  let width = Math.max(1, Math.round(cssWidth * scale));
  let height = Math.max(1, Math.round(cssHeight * scale));
  if (width * height > SHADER_MAX_PIXELS) {
    const shrink = Math.sqrt(SHADER_MAX_PIXELS / (width * height));
    width = Math.max(1, Math.floor(width * shrink));
    height = Math.max(1, Math.floor(height * shrink));
  }
  return { width, height };
}

/** Whether a shader last drawn at `drawnAt` is due again at `now`, at `rate` frames a second. */
export function shaderDue(now: number, drawnAt: number, rate: number): boolean {
  // Half a 120 Hz refresh of slack, so a frame that arrives a hair early still draws.
  return now - drawnAt >= 1000 / rate - 4;
}

/** What a theme file asks of its shader. */
export interface ShaderSettings {
  /** GLSL ES 3.00 fragment source. */
  readonly source: string;
  /** A `data:image/…` URL bound as `u_image`, or null. */
  readonly picture: string | null;
  readonly scale: number;
  readonly rate: number;
  /** Seconds into the pattern to open on, so every launch shows the same composition. */
  readonly start: number;
}

export function sameShader(a: ShaderSettings, b: ShaderSettings): boolean {
  return a.source === b.source && a.picture === b.picture && a.scale === b.scale && a.rate === b.rate
    && a.start === b.start;
}

const VERTEX_SOURCE = `#version 300 es
in vec2 a_position;
void main() { gl_Position = vec4(a_position, 0.0, 1.0); }
`;
const FULL_CANVAS = new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]);

interface ShaderRenderer {
  setSize(width: number, height: number): void;
  draw(seconds: number): void;
  dispose(): void;
}

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader | null {
  const shader = gl.createShader(type);
  if (shader === null) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS) === true) return shader;
  console.warn("[theme] the theme's shader did not compile", gl.getShaderInfoLog(shader));
  gl.deleteShader(shader);
  return null;
}

function link(gl: WebGL2RenderingContext, fragmentSource: string): WebGLProgram | null {
  const vertex = compile(gl, gl.VERTEX_SHADER, VERTEX_SOURCE);
  const fragment = compile(gl, gl.FRAGMENT_SHADER, fragmentSource);
  const program = vertex !== null && fragment !== null ? gl.createProgram() : null;
  if (program !== null && vertex !== null && fragment !== null) {
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragment);
    gl.linkProgram(program);
  }
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (program === null) return null;
  if (gl.getProgramParameter(program, gl.LINK_STATUS) === true) return program;
  console.warn("[theme] the theme's shader did not link", gl.getProgramInfoLog(program));
  gl.deleteProgram(program);
  return null;
}

function bindPicture(gl: WebGL2RenderingContext, program: WebGLProgram, picture: HTMLImageElement): WebGLTexture | null {
  const texture = gl.createTexture();
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, texture);
  // gl_FragCoord counts up from the bottom; flipped rows keep the picture upright.
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, picture);
  // The canvas samples the picture well below its size, which aliases without mipmaps.
  gl.generateMipmap(gl.TEXTURE_2D);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.uniform1i(gl.getUniformLocation(program, "u_image"), 0);
  gl.uniform2f(gl.getUniformLocation(program, "u_imageSize"), picture.naturalWidth, picture.naturalHeight);
  return texture;
}

/** One fragment shader over the whole canvas, or null where WebGL2 or the shader is unavailable. */
function createRenderer(canvas: HTMLCanvasElement, source: string, picture: HTMLImageElement | null): ShaderRenderer | null {
  const gl = canvas.getContext("webgl2", {
    alpha: true, antialias: false, depth: false, stencil: false, premultipliedAlpha: true,
    powerPreference: "low-power",
  });
  if (gl === null) return null;
  const program = link(gl, source);
  if (program === null) return null;
  gl.useProgram(program);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, FULL_CANVAS, gl.STATIC_DRAW);
  const position = gl.getAttribLocation(program, "a_position");
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
  const resolution = gl.getUniformLocation(program, "u_resolution");
  const time = gl.getUniformLocation(program, "u_time");
  gl.clearColor(0, 0, 0, 0);
  const texture = picture === null ? null : bindPicture(gl, program, picture);
  return {
    setSize(width, height) {
      if (canvas.width === width && canvas.height === height) return;
      canvas.width = width;
      canvas.height = height;
      gl.viewport(0, 0, width, height);
      gl.uniform2f(resolution, width, height);
    },
    draw(seconds) {
      gl.uniform1f(time, seconds);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.drawArrays(gl.TRIANGLES, 0, 6);
    },
    dispose() {
      gl.deleteTexture(texture);
      gl.deleteBuffer(buffer);
      gl.deleteProgram(program);
    },
  };
}

/** A shader drawn on its canvas. theme-look.ts moves its clock; it draws when a frame is due. */
export interface MountedShader {
  readonly settings: ShaderSettings;
  /** The shader can never draw here: it did not compile, WebGL2 is missing or
   * its picture did not decode. A lost context that may come back is not this. */
  readonly failed: boolean;
  /** Moves the pattern on by `stepMs` and draws it if its own rate says a frame is due at `now`. */
  advance(stepMs: number, now: number): void;
  /** Draws the pattern as it stands onto `target`, a plain canvas made the same size. */
  copyTo(target: HTMLCanvasElement): void;
  dispose(): void;
}

/**
 * Puts `canvas` to work on a theme's shader. The pattern is drawn once at its
 * starting time straight away (after the picture decodes, when there is one),
 * so a resting backdrop shows it still; a shader that does not compile leaves
 * the canvas hidden and the theme's plain backdrop showing. `drawn` is told
 * each time the shader draws by itself rather than from `advance`: at the
 * start, when its picture arrives and when the canvas changes size.
 */
export function mountThemeShader(canvas: HTMLCanvasElement, settings: ShaderSettings, drawn?: () => void): MountedShader {
  let renderer: ShaderRenderer | null = null;
  let picture: HTMLImageElement | null = null;
  let elapsed = settings.start;
  let drawnAt = Number.NEGATIVE_INFINITY;
  let disposed = false;
  let failed = false;

  const resize = (): void => {
    if (renderer === null) return;
    const size = shaderCanvasSize(canvas.clientWidth, canvas.clientHeight, settings.scale);
    renderer.setSize(size.width, size.height);
    renderer.draw(elapsed);
    drawn?.();
  };
  const start = (): void => {
    if (disposed) return;
    renderer = createRenderer(canvas, settings.source, picture);
    failed = renderer === null;
    canvas.hidden = failed;
    resize();
  };
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(resize) : undefined;
  observer?.observe(canvas);
  const onLost = (event: Event): void => {
    event.preventDefault();
    renderer = null;
  };
  canvas.addEventListener("webglcontextlost", onLost);
  canvas.addEventListener("webglcontextrestored", start);

  if (settings.picture === null) {
    start();
  } else {
    canvas.hidden = true;
    const image = new Image();
    image.src = settings.picture;
    image.decode().then(() => { picture = image; start(); }, () => {
      failed = true;
      console.warn("[theme] the theme's shader picture did not decode");
    });
  }

  return {
    settings,
    get failed() { return failed; },
    advance(stepMs, now) {
      elapsed += stepMs / 1000;
      if (renderer === null || !shaderDue(now, drawnAt, settings.rate)) return;
      drawnAt = now;
      renderer.draw(elapsed);
    },
    copyTo(target) {
      if (renderer === null) return;
      const context = target.getContext("2d");
      if (context === null) return;
      if (target.width !== canvas.width || target.height !== canvas.height) {
        target.width = canvas.width;
        target.height = canvas.height;
      }
      // A WebGL canvas keeps its picture only until it is shown, so it is drawn and copied in one go.
      renderer.draw(elapsed);
      context.clearRect(0, 0, target.width, target.height);
      context.drawImage(canvas, 0, 0);
    },
    dispose() {
      disposed = true;
      observer?.disconnect();
      canvas.removeEventListener("webglcontextlost", onLost);
      canvas.removeEventListener("webglcontextrestored", start);
      renderer?.dispose();
      renderer = null;
    },
  };
}

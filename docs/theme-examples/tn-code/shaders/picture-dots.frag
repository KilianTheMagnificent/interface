#version 300 es
precision highp float;
uniform vec2 u_resolution;
uniform float u_time;
out vec4 fragColor;

float hash12(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
vec3 hash32(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
  p3 += dot(p3, p3.yxz + 33.33);
  return fract((p3.xxy + p3.yzz) * p3.zyx);
}
float vnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(
    mix(hash12(i), hash12(i + vec2(1.0, 0.0)), u.x),
    mix(hash12(i + vec2(0.0, 1.0)), hash12(i + vec2(1.0, 1.0)), u.x),
    u.y
  );
}


float drifting(vec2 p, float period, float seed) {
  float k = u_time / period;
  float epoch = floor(k);
  float f = smoothstep(0.0, 1.0, fract(k));
  float from = vnoise(p + hash12(vec2(epoch, seed)) * 97.0);
  float to = vnoise(p + hash12(vec2(epoch + 1.0, seed)) * 97.0);
  return mix(from, to, f);
}


uniform sampler2D u_image;
uniform vec2 u_imageSize;
vec4 coverSample(vec2 frag) {
  vec2 uv = frag / u_resolution;
  float canvasAspect = u_resolution.x / u_resolution.y;
  float imageAspect = u_imageSize.x / u_imageSize.y;
  if (canvasAspect > imageAspect) uv.y = 0.5 + (uv.y - 0.5) * imageAspect / canvasAspect;
  else uv.x = 0.5 + (uv.x - 0.5) * canvasAspect / imageAspect;
  return texture(u_image, uv);
}

const float PITCH = 4.0;

void main() {
  vec2 px = floor(gl_FragCoord.xy);
  float row = floor(px.y / PITCH);
  float shift = mod(row, 2.0) * PITCH * 0.5;
  float column = floor((px.x + shift) / PITCH);
  vec2 cell = vec2(column, row);
  vec2 origin = vec2(column * PITCH - shift, row * PITCH);
  vec2 o = px - origin;
  vec2 center = origin + PITCH * 0.5;
  float t = u_time;

  float patches = drifting(cell / vec2(36.0, 36.0), 5.0, 1.0) * 0.65
    + drifting(cell / vec2(16.0, 16.0), 3.3, 2.0) * 0.35;
  patches = smoothstep(0.32, 0.8, patches);

  float heading = t * 0.015;
  vec2 k1 = vec2(cos(heading), sin(heading));
  vec2 k2 = vec2(cos(heading + 1.05), sin(heading + 1.05));
  float s1 = sin(dot(center, k1) * 0.016 - t * 0.55);
  float s2 = sin(dot(center, k2) * 0.021 - t * 0.43);
  float crest = min(1.0, 0.6 * pow(0.5 + 0.5 * s1, 3.0) + 0.6 * pow(0.5 + 0.5 * s2, 3.0));

  bool inX = o.x >= 1.0 && o.x <= 2.0;
  bool inY = o.y >= 1.0 && o.y <= 2.0;
  float plus = smoothstep(0.8, 1.0, crest) * step(hash12(cell + 3.1), 0.3);
  float shape = inX && inY ? 1.0 : (inX || inY ? plus : 0.0);

  vec4 texel = coverSample(center);
  vec3 col = min(texel.rgb * (0.82 + 0.12 * patches + 0.3 * crest), 1.0);
  float a = shape * texel.a;
  fragColor = vec4(col * a, a);
}

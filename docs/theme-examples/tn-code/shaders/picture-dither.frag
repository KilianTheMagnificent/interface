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


const float BAYER[16] = float[16](
  0.0, 8.0, 2.0, 10.0,
  12.0, 4.0, 14.0, 6.0,
  3.0, 11.0, 1.0, 9.0,
  15.0, 7.0, 13.0, 5.0
);


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

void main() {
  vec2 cell = floor(gl_FragCoord.xy);
  vec2 p = (cell + 0.5) / u_resolution.y;
  float t = u_time;

  float heading = t * 0.02;
  vec2 k1 = vec2(cos(heading), sin(heading));
  vec2 k2 = vec2(cos(heading + 1.05), sin(heading + 1.05));
  float s1 = sin(dot(p, k1) * 34.0 - t * 0.55);
  float s2 = sin(dot(p, k2) * 26.0 - t * 0.42);
  vec2 ripple = (k1 * s1 + k2 * s2) * 0.004 * u_resolution.y;

  vec4 texel = coverSample(cell + 0.5 + ripple);
  vec3 col = texel.rgb * (0.93 + 0.07 * sin(t * 0.21));
  col += (hash32(cell) - 0.5) * 0.05;

  ivec2 bi = ivec2(mod(cell, 4.0));
  float threshold = (BAYER[bi.x + bi.y * 4] + 0.5) / 16.0;
  vec3 q = clamp(floor(col * 5.0 + threshold) / 5.0, 0.0, 1.0);
  float a = step(threshold, texel.a);
  fragColor = vec4(q * a, a);
}

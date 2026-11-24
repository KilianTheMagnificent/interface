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

const int STOPS = 26;
const float STOP_D[STOPS] = float[STOPS](-0.4000, -0.2000, -0.1400, -0.1000, -0.0800, -0.0650, -0.0550, -0.0450, -0.0350, -0.0250, -0.0150, -0.0050, 0.0050, 0.0150, 0.0300, 0.0450, 0.0600, 0.0800, 0.1200, 0.1600, 0.2000, 0.2400, 0.2800, 0.3200, 0.3600, 0.4400);
const vec3 WEST[STOPS] = vec3[STOPS](
  vec3(0.0549, 0.0510, 0.0471),
  vec3(0.0745, 0.0706, 0.0588),
  vec3(0.0941, 0.0980, 0.0784),
  vec3(0.1176, 0.1216, 0.0941),
  vec3(0.1569, 0.1373, 0.1059),
  vec3(0.4157, 0.1373, 0.0941),
  vec3(0.4941, 0.1922, 0.0627),
  vec3(0.5490, 0.2706, 0.0784),
  vec3(0.5922, 0.3569, 0.1490),
  vec3(0.6157, 0.4314, 0.2510),
  vec3(0.5922, 0.4980, 0.3725),
  vec3(0.5373, 0.5333, 0.4824),
  vec3(0.3961, 0.5020, 0.5490),
  vec3(0.2588, 0.4588, 0.5725),
  vec3(0.1333, 0.4078, 0.5725),
  vec3(0.1098, 0.4157, 0.5961),
  vec3(0.0745, 0.3412, 0.5020),
  vec3(0.0667, 0.2902, 0.4235),
  vec3(0.0549, 0.2627, 0.3686),
  vec3(0.0353, 0.2157, 0.2980),
  vec3(0.0353, 0.1686, 0.2275),
  vec3(0.0353, 0.1333, 0.1765),
  vec3(0.0392, 0.1059, 0.1294),
  vec3(0.0510, 0.0824, 0.0902),
  vec3(0.0549, 0.0667, 0.0627),
  vec3(0.0549, 0.0510, 0.0471)
);
const vec3 NOON[STOPS] = vec3[STOPS](
  vec3(0.0510, 0.0510, 0.0471),
  vec3(0.0706, 0.0667, 0.0588),
  vec3(0.1020, 0.0863, 0.0784),
  vec3(0.1765, 0.1255, 0.1176),
  vec3(0.2471, 0.1373, 0.1176),
  vec3(0.5961, 0.1137, 0.0706),
  vec3(0.7216, 0.1961, 0.0431),
  vec3(0.7529, 0.4627, 0.0431),
  vec3(0.7373, 0.4627, 0.0863),
  vec3(0.7216, 0.4706, 0.1490),
  vec3(0.6157, 0.4706, 0.3255),
  vec3(0.6039, 0.5490, 0.4549),
  vec3(0.5765, 0.5922, 0.5765),
  vec3(0.4863, 0.5882, 0.6471),
  vec3(0.4510, 0.6039, 0.7059),
  vec3(0.2431, 0.4902, 0.6667),
  vec3(0.2196, 0.4588, 0.6431),
  vec3(0.2157, 0.4235, 0.5922),
  vec3(0.2196, 0.3608, 0.4902),
  vec3(0.1451, 0.2902, 0.3882),
  vec3(0.0824, 0.2275, 0.2980),
  vec3(0.0510, 0.1725, 0.2235),
  vec3(0.0471, 0.1294, 0.1608),
  vec3(0.0588, 0.1020, 0.1098),
  vec3(0.0627, 0.0824, 0.0784),
  vec3(0.0588, 0.0588, 0.0549)
);
const vec3 EAST[STOPS] = vec3[STOPS](
  vec3(0.0510, 0.0471, 0.0431),
  vec3(0.0745, 0.0863, 0.0745),
  vec3(0.0745, 0.1059, 0.0980),
  vec3(0.0941, 0.1294, 0.1255),
  vec3(0.1333, 0.1490, 0.1412),
  vec3(0.1647, 0.1608, 0.1569),
  vec3(0.1725, 0.1725, 0.1686),
  vec3(0.1961, 0.1961, 0.1765),
  vec3(0.3686, 0.2627, 0.1922),
  vec3(0.5176, 0.3922, 0.2549),
  vec3(0.5216, 0.4588, 0.3647),
  vec3(0.5137, 0.5098, 0.4745),
  vec3(0.4980, 0.5373, 0.5451),
  vec3(0.4784, 0.5490, 0.5804),
  vec3(0.4157, 0.5294, 0.6039),
  vec3(0.2980, 0.4784, 0.5961),
  vec3(0.2118, 0.4118, 0.5686),
  vec3(0.2314, 0.4118, 0.5569),
  vec3(0.2275, 0.3686, 0.4980),
  vec3(0.1647, 0.3059, 0.4039),
  vec3(0.0941, 0.2431, 0.3255),
  vec3(0.0431, 0.1961, 0.2510),
  vec3(0.0392, 0.1451, 0.1882),
  vec3(0.0627, 0.1098, 0.1137),
  vec3(0.0627, 0.0706, 0.0667),
  vec3(0.0510, 0.0471, 0.0431)
);

vec3 band(float d, float along) {
  int i = 0;
  for (int k = 0; k < STOPS - 1; k++) {
    if (d >= STOP_D[k]) i = k;
  }
  float f = clamp((d - STOP_D[i]) / (STOP_D[i + 1] - STOP_D[i]), 0.0, 1.0);
  vec3 west = mix(WEST[i], WEST[i + 1], f);
  vec3 noon = mix(NOON[i], NOON[i + 1], f);
  vec3 east = mix(EAST[i], EAST[i + 1], f);
  return mix(mix(west, noon, smoothstep(0.11, 0.39, along)), east, smoothstep(0.39, 0.9, along));
}

void main() {
  vec2 cell = floor(gl_FragCoord.xy);
  float aspect = u_resolution.x / u_resolution.y;
  vec2 p = (cell + 0.5) / u_resolution.y;
  float t = u_time;

  float heading = t * 0.02;
  vec2 k1 = vec2(cos(heading), sin(heading));
  vec2 k2 = vec2(cos(heading + 1.05), sin(heading + 1.05));
  float s1 = sin(dot(p, k1) * 34.0 - t * 0.55);
  float s2 = sin(dot(p, k2) * 26.0 - t * 0.42);
  p += (k1 * s1 + k2 * s2) * 0.003;

  vec2 a = vec2(0.0, 0.934);
  vec2 b = vec2(aspect, 0.184);
  vec2 dir = normalize(b - a);
  vec2 rel = p - a;
  float d = rel.y * dir.x - rel.x * dir.y;
  float along = dot(rel, dir) / length(b - a) - 0.06 * sin(t * 0.045);

  vec3 col = band(d, along) * (0.97 + 0.03 * sin(t * 0.21));
  // A shared flicker leaves gaps in the dark lattice; speckle leans red; the band gets color grain.
  col += (hash12(cell + 7.3) - 0.75) * 0.03 + hash32(cell) * vec3(0.026, 0.009, 0.004);
  col += (hash32(cell + 19.1) - 0.5) * 0.045 * smoothstep(0.12, 0.4, dot(col, vec3(0.33)));

  ivec2 bi = ivec2(mod(cell, 4.0));
  float threshold = (BAYER[bi.x + bi.y * 4] + 0.5) / 16.0;
  vec3 q = clamp(floor(col * 5.0 + threshold) / 5.0, 0.0, 1.0);
  fragColor = vec4(q, 1.0);
}

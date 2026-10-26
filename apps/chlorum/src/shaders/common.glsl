#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform float t;       // seconds, already scaled by speed
uniform vec2 res;      // pixels
uniform float bright;  // 0..1
uniform float amount;  // 1..10
uniform vec3 col;      // accent colour
// Random numbers are worked out once, when the wallpaper starts, into a
// 256 x 256 texture (four independent values per texel). Reading them is
// cheap and gives the same picture on every graphics card, unlike the
// usual fract(sin(x) * 43758.5) trick, which NVIDIA computes differently.
uniform sampler2D rnd; // the values, unfiltered
uniform sampler2D smo; // the same values, smoothly blended
// Note: smoothstep(a, b, x) needs a < b. With a >= b GLSL gives no promise,
// and NVIDIA draws nothing: write 1.0 - smoothstep(b, a, x) instead.
vec4 hash4(vec2 p){ return texture2D(rnd, (floor(p) + 0.5) / 256.0); }
float hash(vec2 p){ return hash4(p).x; }
// Smooth value noise in one read: the texture blends neighbours itself.
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return texture2D(smo, (i + f + 0.5) / 256.0).x;
}
const mat2 turn = mat2(0.80, 0.60, -0.60, 0.80);
float fbm(vec2 p){ float v = 0.0, a = 0.5; for (int i = 0; i < 5; i++) { v += a * noise(p); p = turn * p * 2.02 + 17.0; a *= 0.5; } return v; }
// Keeps the left side calm, where Home puts the clock and the app names.
float calm(vec2 uv){ return mix(0.55, 1.0, smoothstep(0.0, 0.7, uv.x)); }

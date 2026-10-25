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
float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}
float fbm(vec2 p){ float v = 0.0, a = 0.5; for (int i = 0; i < 5; i++) { v += a * noise(p); p *= 2.02; a *= 0.5; } return v; }
// Keeps the left side calm, where Home puts the clock and the app names.
float calm(vec2 uv){ return mix(0.55, 1.0, smoothstep(0.0, 0.7, uv.x)); }

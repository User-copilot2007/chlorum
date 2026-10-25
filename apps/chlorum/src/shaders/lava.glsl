// Wax rises, stretches and merges like in a lava lamp.
void main(){
  vec2 uv = gl_FragCoord.xy / res;
  float aspect = res.x / res.y;
  float f = 0.0;
  for (int i = 0; i < 10; i++) {
    float fi = float(i);
    if (fi >= amount) break;
    float sp = 0.10 + 0.07 * fract(fi * 0.37);
    float ph = fi * 2.1;
    vec2 c = vec2(0.34 + 0.56 * fract(sin(fi * 12.9898) * 43.7585),
                  0.5 + 0.62 * sin(t * sp + ph));
    c.x += 0.06 * sin(t * 0.11 + fi * 1.7);
    float r = 0.075 + 0.07 * fract(fi * 0.618);
    vec2 d = uv - c; d.x *= aspect;
    // stretched while moving fast
    d.y *= 1.0 - 0.35 * abs(cos(t * sp + ph));
    f += r * r / max(dot(d, d), 1e-5);
  }
  // pools of wax at the top and bottom of the lamp
  f += 0.010 / max(uv.y + 0.02, 0.001) + 0.010 / max(1.02 - uv.y, 0.001);
  float edge = smoothstep(0.95, 1.05, f);
  float glow = clamp(f * 0.30, 0.0, 1.0);
  vec3 wax = col * (0.72 + 0.38 * uv.y) - vec3(0.0, 0.12, 0.0) * (1.0 - uv.y);
  vec3 c3 = mix(col * glow * glow * 0.20, wax, edge);
  gl_FragColor = vec4(c3 * bright * calm(uv), 1.0);
}

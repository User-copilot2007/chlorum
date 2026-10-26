void main(){
  vec2 uv = gl_FragCoord.xy / res;
  vec2 o = vec2(0.64, 1.18);
  vec2 d = uv - o; d.x *= res.x / res.y;
  float a = atan(d.x, -d.y);
  float dist = length(d);
  float v = 0.0;
  for (int i = 0; i < 10; i++) {
    float fi = float(i);
    if (fi >= amount + 2.0) break;
    float center = sin(t * (0.10 + 0.03 * fi) + fi * 1.7) * 0.55;
    float width = 0.025 + 0.03 * fract(fi * 0.618);
    v += (1.0 - smoothstep(0.0, width, abs(a - center))) * (0.5 + 0.5 * noise(vec2(dist * 6.0 - t * 0.4, fi)));
  }
  float fall = exp(-dist * 1.1);
  vec3 c3 = col * v * fall * 0.5 + col * 0.04 * fall;
  gl_FragColor = vec4(c3 * bright * calm(uv), 1.0);
}

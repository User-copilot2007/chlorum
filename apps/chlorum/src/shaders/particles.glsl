// Specks of light drifting up in three layers, the nearest ones larger.
void main(){
  vec2 uv = gl_FragCoord.xy / res;
  float aspect = res.x / res.y;
  vec3 c3 = vec3(0.0);
  for (int l = 0; l < 3; l++) {
    float fl = float(l);
    float scale = 6.0 + fl * 5.0 + amount;
    vec2 p = vec2(uv.x * aspect, uv.y) * scale;
    p.y -= t * (0.25 + fl * 0.18);
    p.x += sin(t * 0.3 + fl) * 0.3;
    vec2 cell = floor(p), f = fract(p);
    vec4 r = hash4(cell + vec2(fl * 61.0, fl * 29.0));
    if (r.x > 0.55) {
      vec2 c = 0.2 + 0.6 * r.yz;
      float rad = 0.05 + 0.06 * (2.0 - fl) / 2.0;
      float d = length(f - c);
      float m = 1.0 - smoothstep(rad * 0.4, rad, d);
      float tw = 0.6 + 0.4 * sin(t * 2.0 + r.w * 40.0);
      c3 += col * (m) * tw * (0.35 + 0.3 * (2.0 - fl));
    }
  }
  gl_FragColor = vec4(c3 * bright * calm(uv), 1.0);
}

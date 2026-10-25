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
    float h = hash(cell + fl * 17.0);
    if (h > 0.55) {
      vec2 c = vec2(0.2 + 0.6 * hash(cell + 3.1), 0.2 + 0.6 * hash(cell + 7.7));
      float r = 0.05 + 0.06 * (2.0 - fl) / 2.0;
      float m = smoothstep(r, r * 0.4, length(f - c));
      float tw = 0.6 + 0.4 * sin(t * 2.0 + h * 40.0);
      c3 += col * m * tw * (0.35 + 0.3 * (2.0 - fl));
    }
  }
  gl_FragColor = vec4(c3 * bright * calm(uv), 1.0);
}

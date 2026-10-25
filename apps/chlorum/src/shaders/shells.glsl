// Chlorine: 17 electrons in shells of 2, 8 and 7; one place stays open.
vec2 ell(float ang, float r, float tilt){
  vec2 e = vec2(cos(ang) * r * 1.7, sin(ang) * r * 0.55);
  float c = cos(tilt), s = sin(tilt);
  return vec2(e.x * c - e.y * s, e.x * s + e.y * c);
}
void main(){
  vec2 uv = gl_FragCoord.xy / res;
  vec2 p = (gl_FragCoord.xy - res * vec2(0.62, 0.5)) / res.y;
  float tilt = -0.32;
  vec3 c3 = vec3(0.0);
  float radii[3]; radii[0] = 0.12; radii[1] = 0.22; radii[2] = 0.32;
  float counts[3]; counts[0] = 2.0; counts[1] = 8.0; counts[2] = 8.0;
  float speeds[3]; speeds[0] = 0.9; speeds[1] = 0.45; speeds[2] = 0.28;
  for (int s = 0; s < 3; s++) {
    // the ring
    float c = cos(-tilt), sn = sin(-tilt);
    vec2 q = vec2(p.x * c - p.y * sn, p.x * sn + p.y * c);
    float k = length(q / vec2(1.7, 0.55));
    c3 += vec3(0.16) * smoothstep(0.004, 0.0, abs(k - radii[s]) * 0.55);
    for (int i = 0; i < 8; i++) {
      float fi = float(i);
      if (fi >= counts[s]) break;
      float ang = t * speeds[s] + fi / counts[s] * 6.2831;
      vec2 e = ell(ang, radii[s], tilt);
      float d = length(p - e);
      if (s == 2 && i == 0) {
        c3 += col * 0.6 * smoothstep(0.003, 0.0, abs(d - 0.011)) * (0.5 + 0.5 * sin(t * 3.0));
      } else {
        c3 += col * smoothstep(0.011, 0.007, d) + col * 0.25 * exp(-d * 60.0);
      }
    }
  }
  float dn = length(p);
  c3 += col * smoothstep(0.045, 0.041, dn) + col * 0.15 * exp(-dn * 12.0);
  gl_FragColor = vec4(c3 * bright, 1.0);
}

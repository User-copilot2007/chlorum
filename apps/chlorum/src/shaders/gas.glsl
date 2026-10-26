// Chlorine gas: yellow-green clouds that fold slowly into each other.
void main(){
  vec2 uv = gl_FragCoord.xy / res;
  vec2 p = uv * vec2(res.x / res.y, 1.0) * (1.2 + amount * 0.12);
  vec2 q = vec2(fbm(p + vec2(0.0, t * 0.05)), fbm(p + vec2(5.2, 1.3) - t * 0.04));
  float n = fbm(p + 2.5 * q + vec2(t * 0.03, 0.0));
  vec3 green = mix(col, vec3(0.78, 0.90, 0.25), 0.6);
  float cloud = smoothstep(0.30, 0.80, n);
  vec3 c3 = mix(col, green, q.y) * (0.06 + 0.60 * cloud * cloud);
  gl_FragColor = vec4(c3 * bright * calm(uv), 1.0);
}

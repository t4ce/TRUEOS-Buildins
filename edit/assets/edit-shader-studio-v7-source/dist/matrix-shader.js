// Append this body to the studio's SHARED_GLSL. The rain reads real glyph coverage.
export const MATRIX_BODY = `
float matrixRow(float digit,float row){
  vec4 a; vec3 b;
  if(digit<0.5){a=vec4(14.,17.,19.,21.);b=vec3(25.,17.,14.);}
  else if(digit<1.5){a=vec4(4.,12.,4.,4.);b=vec3(4.,4.,14.);}
  else if(digit<2.5){a=vec4(14.,17.,1.,2.);b=vec3(4.,8.,31.);}
  else if(digit<3.5){a=vec4(14.,17.,1.,6.);b=vec3(1.,17.,14.);}
  else if(digit<4.5){a=vec4(2.,6.,10.,18.);b=vec3(31.,2.,2.);}
  else if(digit<5.5){a=vec4(31.,16.,30.,1.);b=vec3(1.,17.,14.);}
  else if(digit<6.5){a=vec4(6.,8.,16.,30.);b=vec3(17.,17.,14.);}
  else if(digit<7.5){a=vec4(31.,1.,2.,4.);b=vec3(8.,8.,8.);}
  else if(digit<8.5){a=vec4(14.,17.,17.,14.);b=vec3(17.,17.,14.);}
  else if(digit<9.5){a=vec4(14.,17.,17.,15.);b=vec3(1.,2.,12.);}
  else if(digit<10.5){a=vec4(31.,4.,4.,14.);b=vec3(4.,4.,8.);}
  else if(digit<11.5){a=vec4(1.,1.,17.,17.);b=vec3(10.,4.,0.);}
  else if(digit<12.5){a=vec4(31.,1.,2.,4.);b=vec3(8.,16.,0.);}
  else if(digit<13.5){a=vec4(4.,31.,4.,14.);b=vec3(21.,4.,0.);}
  else if(digit<14.5){a=vec4(0.,21.,21.,21.);b=vec3(17.,1.,2.);}
  else{a=vec4(16.,16.,31.,1.);b=vec3(1.,2.,4.);}
  if(row<0.5)return a.x;
  if(row<1.5)return a.y;
  if(row<2.5)return a.z;
  if(row<3.5)return a.w;
  if(row<4.5)return b.x;
  if(row<5.5)return b.y;
  return b.z;
}
float matrixGlyph(vec2 cell,float digit){
  vec2 p=(cell-vec2(.16,.11))/vec2(.68,.78)*vec2(5.,7.);
  if(p.x<0.||p.x>=5.||p.y<0.||p.y>=7.)return 0.;
  float bits=matrixRow(digit,floor(p.y));
  float bit=mod(floor(bits/exp2(4.-floor(p.x))),2.);
  vec2 f=fract(p);
  float inset=step(.06,f.x)*step(f.x,.96)*step(.04,f.y)*step(f.y,.97);
  return bit*inset;
}
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 maskSize=max(iChannelResolution[0].xy,vec2(1.));
  // Dimensions follow the captured CSS-pixel image, so quality does not change glyph size.
  vec2 cellSize=vec2(15.,22.);
  vec2 grid=vec2(uv.x,1.-uv.y)*maskSize/cellSize;
  vec2 id=floor(grid), local=fract(grid);
  float column=hash21(vec2(id.x,17.23));
  float rows=maskSize.y/cellSize.y;
  float speed=5.5+column*9.;
  float tailLength=8.+hash21(vec2(id.x,41.7))*13.;
  float cycle=rows+tailLength+4.;
  float head=mod(iTime*speed+column*cycle,cycle);
  float behind=head-grid.y;
  float tail=step(0.,behind)*step(behind,tailLength)*pow(max(0.,1.-behind/tailLength),1.45);
  float leading=step(0.,behind)*(1.-smoothstep(.2,1.3,behind));

  // Project the real text/feedback onto each rain column. No image means no streams.
  // Eight rows × three texture fetches = 24 fetches for this emitter lookup.
  float emitter=0.;
  float sx=(id.x+.5)*cellSize.x/maskSize.x;
  for(int j=0;j<8;j++){
    float sy=(float(j)+.5)/8.;
    vec2 source=vec2(sx,sy);
    float glyph=sampleText(source)+sampleText(source+vec2(2./maskSize.x,0.));
    vec3 field=sampleField(source);
    emitter+=glyph*.20+field.r*.25+field.g*.65;
  }
  float fromText=1.-exp(-emitter*7.2);
  float gate=mix(.82,fromText,u_textInfluence);

  float swap=floor(iTime*(2.+column*5.)+id.y*.13);
  float code=floor(hash21(id+vec2(swap,column*31.))*16.);
  float glyph=matrixGlyph(local,code);
  float dropout=step(.12,hash21(id+vec2(9.2,floor(iTime*.45))));
  float beam=exp(-abs(local.x-.5)*14.)*tail*.055;
  float energy=(glyph*tail*dropout+beam)*gate;

  // Actual document glyph fragments also fall at each column's own speed.
  vec2 carriedUV=vec2(uv.x,fract(uv.y+iTime*(.035+column*.035)));
  float carried=sampleText(carriedUV)*tail*gate;
  vec3 localField=sampleField(uv);
  float localGlow=(localField.r*.10+localField.g*.10)*u_textInfluence;

  vec3 green=vec3(.045,1.,.28);
  vec3 bright=vec3(.72,1.,.80);
  if(u_palette>.5&&u_palette<1.5){green=vec3(.08,.96,.61);bright=vec3(.76,1.,.91);}
  else if(u_palette>1.5&&u_palette<2.5){green=vec3(1.,.64,.11);bright=vec3(1.,.94,.66);}
  else if(u_palette>2.5){green=vec3(.66,.82,.73);bright=vec3(.94,1.,.97);}
  vec3 color=background();
  color+=green*(energy*(.82+.60*tail)+carried*.42+localGlow);
  color+=bright*glyph*leading*gate*1.05;
  fragColor=vec4(color,1.);
}
`;

export const MATRIX_PRESET = Object.freeze({
  id: 'matrix',
  name: 'Matrix rain',
  category: 'DIGITAL RAIN',
  description: 'Captured letters power falling glyph streams and luminous green heads.',
});

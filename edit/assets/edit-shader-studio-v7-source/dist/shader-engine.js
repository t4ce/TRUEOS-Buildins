// Text is an independent single-channel GPU input. No DOM editor access lives here.
import { MATRIX_BODY, MATRIX_PRESET } from './matrix-shader.js';

export const SHARED_GLSL = `
float hash21(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}
vec3 ink(float n) {
  vec3 a=vec3(0.48,0.31,0.96), b=vec3(0.19,0.78,0.79), c=vec3(0.98,0.63,0.48);
  if(u_palette>0.5&&u_palette<1.5){a=vec3(0.12,0.57,0.44);b=vec3(0.57,0.92,0.69);c=vec3(0.27,0.69,0.79);}
  else if(u_palette>1.5&&u_palette<2.5){a=vec3(0.82,0.29,0.19);b=vec3(1.0,0.71,0.33);c=vec3(0.68,0.32,0.47);}
  else if(u_palette>2.5){a=vec3(0.36,0.41,0.47);b=vec3(0.72,0.78,0.83);c=vec3(0.93,0.95,0.96);}
  return n>0.84?mix(b,c,smoothstep(0.84,1.0,n)):mix(a,b,smoothstep(0.0,0.84,n));
}
float inBounds(vec2 uv){return step(0.0,uv.x)*step(uv.x,1.0)*step(0.0,uv.y)*step(uv.y,1.0);}
float sampleText(vec2 uv){return texture2D(iChannel0,clamp(uv,0.0,1.0)).r*inBounds(uv);}
vec3 sampleField(vec2 uv){return texture2D(iChannel1,clamp(uv,0.0,1.0)).rgb*inBounds(uv);}
float glyphCloud(vec2 uv,float radius){
  vec2 px=vec2(radius)/max(iChannelResolution[0].xy,vec2(1.0));
  float v=sampleText(uv)*0.20;
  v+=(sampleText(uv+vec2(px.x,0.0))+sampleText(uv-vec2(px.x,0.0))+sampleText(uv+vec2(0.0,px.y))+sampleText(uv-vec2(0.0,px.y)))*0.12;
  v+=(sampleText(uv+px)+sampleText(uv-px)+sampleText(uv+vec2(px.x,-px.y))+sampleText(uv+vec2(-px.x,px.y)))*0.08;
  return v;
}
vec3 background(){return vec3(0.018,0.024,0.039);}
float influence(float value){return mix(0.28,value,u_textInfluence);}
`;

const AURORA=SHARED_GLSL+`
// Each glyph becomes an emitter. Long samples pull its coverage into light curtains.
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 p=uv; float t=iTime*0.23;
  p.x+=sin(uv.y*9.0+t)*0.016+sin(uv.y*23.0-t*0.7)*0.006;
  float ribbon=0.0,shimmer=0.0;
  for(int j=0;j<12;j++){
    float f=float(j)/11.0;
    vec2 q=p+vec2(sin(f*6.0+t+uv.x*8.0)*0.012,f*0.26-0.055);
    vec3 field=sampleField(q);
    vec2 spread=vec2((2.5+f*5.0)/max(iChannelResolution[0].x,1.0),0.0);
    float glyph=sampleText(q)*0.5+(sampleText(q+spread)+sampleText(q-spread))*0.25;
    ribbon+=(field.r*0.82+field.g*0.65+glyph*0.7)*exp(-f*2.0);
    shimmer+=glyph*exp(-f*2.5)*(0.6+0.4*sin(f*22.0-t*3.0));
  }
  ribbon=1.0-exp(-ribbon*0.68);
  float edge=sampleText(uv);
  vec3 color=background();
  color+=ink(0.17+0.64*uv.y+sin(uv.x*5.0+t)*0.12)*influence(ribbon)*0.95;
  color+=ink(0.83)*shimmer*0.27*u_textInfluence;
  color+=ink(0.95)*edge*0.17*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;
const WAVE=SHARED_GLSL+`
// Outward offsets sample the actual letter silhouettes as travelling wave fronts.
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 aspect=vec2(iResolution.y/iResolution.x,1.0);
  float t=iTime*0.45; float ripples=0.0;
  for(int ring=0;ring<4;ring++){
    float phase=fract(u_maskAge*0.25+float(ring)*0.25);
    float radius=0.009+phase*0.075;
    float shell=0.0;
    for(int k=0;k<8;k++){
      float angle=float(k)*0.78539816;
      vec2 q=uv+vec2(cos(angle),sin(angle))*radius*aspect;
      vec3 memory=sampleField(q);
      shell+=sampleText(q)+memory.r*0.3+memory.b*0.2;
    }
    ripples+=shell*0.19*(1.0-phase)*(.65+.35*cos(phase*25.0));
  }
  float center=sampleField(uv).g;
  vec3 color=background()+ink(0.45+sin(t+uv.y*8.0)*0.3)*influence(min(1.0,ripples))*1.05;
  color+=ink(0.9)*center*0.42*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;
const CUBES=SHARED_GLSL+`
// Every tile samples glyph occupancy: type to raise an isometric extrusion.
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  float cell=18.0;
  vec2 grid=fragCoord/cell;
  vec2 id=floor(grid), q=fract(grid)-0.5;
  vec2 center=(id+0.5)*cell/iResolution.xy;
  float raw=glyphCloud(center,5.0);
  vec3 memory=sampleField(center);
  float strength=clamp(raw*1.65+memory.r*1.7+memory.g,0.0,1.0);
  float n=hash21(id);
  float amount=influence(strength);
  float w=.44, h=.24;
  float topY=.20+sin(iTime*.4+n*6.0)*.025;
  float depth=.08+amount*.48;
  vec2 r=q-vec2(0.0,topY);
  float lid=1.0-smoothstep(.97,1.03,abs(r.x)/w+abs(r.y)/h);
  float sideTop=-h+abs(r.x)*h/w;
  float side=step(abs(r.x),w)*step(r.y,sideTop)*step(sideTop-depth,r.y);
  vec3 pigment=ink(.12+.8*hash21(id+vec2(8.0)));
  vec3 face=pigment*(r.x<0.0?.40:.70);
  vec3 color=background();
  color=mix(color,face,side*amount);
  color=mix(color,mix(pigment,vec3(.85,.89,.99),.15),lid*amount);
  color+=pigment*sampleText(uv)*.09*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;
const CONTOUR=SHARED_GLSL+`
// Level sets of the diffusing text field, not procedural noise contours.
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 drift=vec2(sin(uv.y*13.0+iTime*.24),cos(uv.x*9.0-iTime*.2))*.004;
  vec3 field=sampleField(uv+drift);
  float density=field.r*.60+field.g*.7+glyphCloud(uv,7.0)*.22;
  float bands=density*18.0-iTime*.12;
  float distance=abs(fract(bands+.5)-.5);
  float contour=1.0-smoothstep(.035,.12,distance);
  float presence=smoothstep(.008,.12,density);
  vec3 color=background()+ink(.22+density*.6)*influence(presence)*(.17+contour*.73);
  color+=ink(.92)*sampleText(uv)*.14*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;
const DITHER=SHARED_GLSL+`
float bayer2(vec2 p){return mod(2.0*p.x+3.0*p.y,4.0);}
float bayer4(vec2 p){p=mod(floor(p),4.0);return (4.0*bayer2(mod(p,2.0))+bayer2(floor(p/2.0))+.5)/16.0;}
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 q=uv+vec2(sin(uv.y*13.0+iTime*.35)*.014,cos(uv.x*12.0-iTime*.2)*.008);
  vec3 field=sampleField(q);
  float density=clamp(field.r*.72+field.g*.9+glyphCloud(q,4.0)*.4,0.0,1.0);
  float threshold=bayer4(fragCoord/2.0);
  float dots=step(threshold,influence(density));
  vec3 color=background()+ink(.18+uv.y*.65+sin(iTime*.12)*.1)*dots*(.35+density*.65);
  fragColor=vec4(color,1.0);
}
`;
const ECHO=SHARED_GLSL+`
// Old glyph energy drifts and persists in the ping-pong field after a new snapshot.
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec3 trail=sampleField(uv);
  float echo=0.0;
  for(int i=1;i<=7;i++){
    float f=float(i);
    vec2 offset=vec2(sin(iTime*.24+f*.3)*.004*f,.009*f);
    echo+=sampleField(uv+offset).r*exp(-f*.25)*.35;
  }
  vec3 color=background()+ink(.10)*influence(min(1.0,trail.r+echo))*.85;
  color+=ink(.72)*trail.g*.95*u_textInfluence;
  color+=ink(.96)*trail.b*.65*u_textInfluence;
  color+=ink(.9)*sampleText(uv)*.15*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;
export const LIFE_SEED_GLSL=`
uniform sampler2D u_glyphs;
uniform vec2 u_resolution;
// A cell spans about three CSS pixels. Nine samples retain thin antialiased strokes.
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  vec2 reach=vec2(.34)/u_resolution;
  float coverage=texture2D(u_glyphs,uv).r;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      coverage=max(coverage,texture2D(u_glyphs,uv+vec2(float(x),float(y))*reach).r);
    }
  }
  float alive=step(.18,coverage);
  gl_FragColor=vec4(alive,alive,alive,1.0);
}
`;

export const LIFE_EMIT_GLSL=`
uniform sampler2D u_previous;
uniform sampler2D u_emission;
uniform vec2 u_resolution;
// One-shot new ink is added to the living world. Nothing outside it is erased.
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  vec2 reach=vec2(.34)/u_resolution;
  float coverage=texture2D(u_emission,uv).r;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      coverage=max(coverage,texture2D(u_emission,uv+vec2(float(x),float(y))*reach).r);
    }
  }
  float emitted=step(.18,coverage);
  vec3 previous=texture2D(u_previous,uv).rgb;
  float born=emitted*(1.0-step(.5,previous.r));
  gl_FragColor=vec4(max(previous.r,emitted),max(previous.g,emitted),max(previous.b,born),1.0);
}
`;

export const LIFE_STEP_GLSL=`
uniform sampler2D u_previous;
uniform vec2 u_resolution;
uniform float u_trailDecay;
// Finite dead boundaries. Only the binary red channel participates in B3/S23.
float aliveAt(vec2 cell){
  if(cell.x<0.0||cell.y<0.0||cell.x>=u_resolution.x||cell.y>=u_resolution.y)return 0.0;
  return step(.5,texture2D(u_previous,(cell+.5)/u_resolution).r);
}
void main(){
  vec2 cell=floor(gl_FragCoord.xy);
  float neighbors=0.0;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      if(x!=0||y!=0)neighbors+=aliveAt(cell+vec2(float(x),float(y)));
    }
  }
  vec3 previous=texture2D(u_previous,(cell+.5)/u_resolution).rgb;
  float alive=step(.5,previous.r);
  float birth=step(2.5,neighbors)*(1.0-step(3.5,neighbors));
  float survival=alive*step(1.5,neighbors)*(1.0-step(3.5,neighbors));
  float next=max(birth,survival);
  // Trails and birth highlights are display-only; neither feeds the life rule.
  float trail=max(next,max(0.0,previous.g*u_trailDecay-1.0/255.0));
  gl_FragColor=vec4(next,trail,next*(1.0-alive),1.0);
}
`;


// Color is a separate RGBA8 field: occupancy/trails/birth remain in iChannel2.
// At an antialiased edge the straight RGB bytes remain the font's full color.
export const LIFE_COLOR_SEED_GLSL=`
uniform sampler2D u_glyphColors;
uniform sampler2D u_state;
uniform vec2 u_resolution;
uniform vec2 u_glyphResolution;
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  if(texture2D(u_state,uv).r<.5){gl_FragColor=vec4(0.0);return;}
  vec2 reach=vec2(.34)/u_resolution;
  vec4 chosen=vec4(220.0/255.0,226.0/255.0,232.0/255.0,0.0);
  // Inspect the source pixels contributing to the exact nine-tap alpha footprint.
  // Nearest input keeps transparent edges from darkening or blending font RGB.
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      vec2 tap=uv+vec2(float(x),float(y))*reach;
      vec2 base=floor(tap*u_glyphResolution-.5);
      for(int j=0;j<2;j++){
        for(int i=0;i<2;i++){
          vec2 source=(base+vec2(float(i),float(j))+.5)/u_glyphResolution;
          vec4 glyph=texture2D(u_glyphColors,source);
          vec2 weight=max(vec2(0.0),1.0-abs(tap*u_glyphResolution-(base+vec2(float(i),float(j))+.5)));
          float score=glyph.a*weight.x*weight.y;
          if(score>chosen.a)chosen=vec4(glyph.rgb,score);
        }
      }
    }
  }
  gl_FragColor=vec4(chosen.rgb,1.0);
}
`;

export const LIFE_COLOR_EMIT_GLSL=`
uniform sampler2D u_previous;
uniform sampler2D u_previousColors;
uniform sampler2D u_emission;
uniform sampler2D u_glyphColors;
uniform vec2 u_resolution;
uniform vec2 u_glyphResolution;
vec3 colorBytes(vec3 color){return floor(color*255.0+.5);}
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  vec2 reach=vec2(.34)/u_resolution;
  float coverage=texture2D(u_emission,uv).r;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      coverage=max(coverage,texture2D(u_emission,uv+vec2(float(x),float(y))*reach).r);
    }
  }
  vec4 previous=texture2D(u_previousColors,uv);
  if(coverage<.18){gl_FragColor=previous;return;}
  vec3 incoming=vec3(220.0,226.0,232.0)/255.0;
  float strongest=0.0;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      vec2 tap=uv+vec2(float(x),float(y))*reach;
      vec2 base=floor(tap*u_glyphResolution-.5);
      for(int j=0;j<2;j++){
        for(int i=0;i<2;i++){
          vec2 source=(base+vec2(float(i),float(j))+.5)/u_glyphResolution;
          // Read delta alpha at an exact pixel center, then take its full new RGB.
          vec2 weight=max(vec2(0.0),1.0-abs(tap*u_glyphResolution-(base+vec2(float(i),float(j))+.5)));
          float added=texture2D(u_emission,source).r*weight.x*weight.y;
          if(added>strongest){strongest=added;incoming=texture2D(u_glyphColors,source).rgb;}
        }
      }
    }
  }
  vec3 color=colorBytes(incoming);
  if(texture2D(u_previous,uv).r>=.5)color=mod(color+colorBytes(previous.rgb),256.0);
  gl_FragColor=vec4(color/255.0,1.0);
}
`;

export const LIFE_COLOR_STEP_GLSL=`
uniform sampler2D u_previous;
uniform sampler2D u_next;
uniform sampler2D u_previousColors;
uniform vec2 u_resolution;
vec3 colorBytes(vec3 color){return floor(color*255.0+.5);}
void main(){
  vec2 cell=floor(gl_FragCoord.xy),uv=(cell+.5)/u_resolution;
  vec4 previous=texture2D(u_previousColors,uv);
  // Survivors keep their color, and dying cells leave that color in their trail.
  if(texture2D(u_next,uv).b<.5){gl_FragColor=previous;return;}
  vec3 first=vec3(0.0),sum=vec3(0.0);
  float count=0.0,same=1.0;
  for(int y=-1;y<=1;y++){
    for(int x=-1;x<=1;x++){
      vec2 neighbor=cell+vec2(float(x),float(y));
      if((x!=0||y!=0)&&neighbor.x>=0.0&&neighbor.y>=0.0&&neighbor.x<u_resolution.x&&neighbor.y<u_resolution.y){
        vec2 at=(neighbor+.5)/u_resolution;
        if(texture2D(u_previous,at).r>=.5){
          vec3 color=colorBytes(texture2D(u_previousColors,at).rgb);
          if(count<.5)first=color;
          else if(any(greaterThan(abs(color-first),vec3(.5))))same=0.0;
          sum+=color;count+=1.0;
        }
      }
    }
  }
  // A single lineage keeps its font color. Mixed lineages add in the byte ring.
  gl_FragColor=vec4((same>.5?first:mod(sum,256.0))/255.0,1.0);
}
`;

export const LIFE_GLSL=SHARED_GLSL+`
// iChannel2 holds Conway state; iChannel3 holds its inherited glyph RGB.
vec3 lifeHalo(vec2 uv){
  uv=clamp(uv,0.0,1.0);
  return texture2D(iChannel3,uv).rgb*texture2D(iChannel2,uv).g;
}
void mainImage(out vec4 fragColor,in vec2 fragCoord){
  vec2 uv=fragCoord/iResolution.xy;
  vec2 grid=max(iChannelResolution[2].xy,vec2(1.0));
  vec2 p=uv*grid;
  vec2 center=(floor(p)+.5)/grid;
  vec3 state=texture2D(iChannel2,center).rgb;
  vec3 pigment=texture2D(iChannel3,center).rgb;
  vec2 local=abs(fract(p)-.5);
  float tile=1.0-smoothstep(.32,.49,max(local.x,local.y));
  vec2 px=1.0/grid;
  vec3 halo=lifeHalo(center+vec2(px.x,0.0))+lifeHalo(center-vec2(px.x,0.0));
  halo+=lifeHalo(center+vec2(0.0,px.y))+lifeHalo(center-vec2(0.0,px.y));
  float strength=clamp((state.r*.85+state.g*.15)*tile*u_textInfluence,0.0,1.0);
  vec3 color=mix(background(),pigment,strength);
  // Birth glow is the cell's own hue; it never washes syntax colors toward white.
  color+=(halo*.018+pigment*state.b*tile*.10)*u_textInfluence;
  fragColor=vec4(color,1.0);
}
`;

export const PRESETS=Object.freeze([
  {id:'aurora',name:'Aurora',category:'GLYPH LIGHT',description:'Your letters stretch into flowing curtains of light.',source:AURORA},
  {id:'wave',name:'Wave',category:'PROPAGATION',description:'Wave fronts spread out from the shapes you type.',source:WAVE},
  {id:'cubes',name:'Cubes',category:'EXTRUSION',description:'Glyph coverage raises a field of tiny isometric blocks.',source:CUBES},
  {id:'contour',name:'Contours',category:'TEXT FIELD',description:'Contour lines trace a diffusing image of your text.',source:CONTOUR},
  {id:'dither',name:'Dither',category:'HALFTONE',description:'Your text becomes a drifting, ordered pixel field.',source:DITHER},
  {id:'echo',name:'Echo',category:'FEEDBACK',description:'Previous text leaves luminous trails as the input changes.',source:ECHO},
  {id:'life',name:'Game of Life',category:'B3 / S23',description:'New glyphs join Conway’s world. Scrolling refreshes it from the visible text.',source:LIFE_GLSL},
  {...MATRIX_PRESET,source:SHARED_GLSL+MATRIX_BODY}
]);

export const FEEDBACK_GLSL=`
uniform sampler2D u_previous;
uniform sampler2D u_glyphs;
uniform vec2 u_resolution;
uniform float u_time;
uniform float u_delta;
uniform float u_feedback;
uniform float u_inject;
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  float dt=clamp(u_delta*60.0,0.0,3.0);
  vec2 px=1.0/u_resolution;
  vec2 flow=vec2(sin(uv.y*11.0+u_time*.21)*.23,0.28+cos(uv.x*9.0-u_time*.17)*.10)*px*dt;
  vec2 q=clamp(uv-flow,0.0,1.0);
  vec3 c=texture2D(u_previous,q).rgb;
  vec3 n=texture2D(u_previous,q+vec2(px.x,0.0)*1.6).rgb+texture2D(u_previous,q-vec2(px.x,0.0)*1.6).rgb;
  n+=texture2D(u_previous,q+vec2(0.0,px.y)*1.6).rgb+texture2D(u_previous,q-vec2(0.0,px.y)*1.6).rgb;
  vec3 wide=texture2D(u_previous,q+px*3.2).rgb+texture2D(u_previous,q-px*3.2).rgb;
  wide+=texture2D(u_previous,q+vec2(px.x,-px.y)*3.2).rgb+texture2D(u_previous,q+vec2(-px.x,px.y)*3.2).rgb;
  float glyph=texture2D(u_glyphs,uv).r;
  float decay=pow(mix(.948,.997,u_feedback),dt);
  float broadDecay=pow(mix(.963,.999,u_feedback),dt);
  // Retained glyphs emit continuously. New revisions also add a short-lived pulse.
  float energy=mix(c.r,n.r*.25,clamp(dt*.48,0.0,1.0))*decay;
  energy=max(energy+glyph*.055*dt,glyph*.52*u_inject);
  float broad=mix(c.g,wide.g*.25,clamp(dt*.55,0.0,1.0))*broadDecay;
  broad=max(broad+glyph*.022*dt,glyph*.24*u_inject);
  float pulse=mix(c.b,n.b*.25,clamp(dt*.30,0.0,1.0))*pow(.973,dt);
  pulse=max(pulse,glyph*u_inject);
  gl_FragColor=vec4(clamp(vec3(energy,broad,pulse),0.0,1.0),1.0);
}
`;

export const UNIFORM_NAMES=['iResolution','iTime','iMouse','iFrame','u_palette','iChannel0','iChannel1','iChannel2','iChannel3','iChannelResolution[0]','iChannelResolution[1]','iChannelResolution[2]','iChannelResolution[3]','u_maskRevision','u_emissionRevision','u_maskAge','u_textInfluence','u_lifeGeneration'];
export function vertexSource(webgl2=false){return webgl2?'#version 300 es\nin vec2 a_position;void main(){gl_Position=vec4(a_position,0.0,1.0);}':'attribute vec2 a_position;void main(){gl_Position=vec4(a_position,0.0,1.0);}';}
export function fragmentSource(source,webgl2=false,precision='highp',feedback=false){
  let body=source.replace(/^\s*#version[^\n]*\n?/m,'');
  if(webgl2) body=body.replace(/\btexture2D\s*\(/g,'texture(').replace(/\bgl_FragColor\b/g,'studioOutput');
  const header=(webgl2?'#version 300 es\n':'')+`precision ${precision} float;\n`+(webgl2?'out vec4 studioOutput;\n':'')+(feedback?'':`uniform vec3 iResolution;\nuniform float iTime;\nuniform vec4 iMouse;\nuniform float iFrame;\nuniform float u_palette;\nuniform sampler2D iChannel0;\nuniform sampler2D iChannel1;\nuniform sampler2D iChannel2;\nuniform sampler2D iChannel3;\nuniform vec3 iChannelResolution[4];\nuniform float u_maskRevision;\nuniform float u_emissionRevision;\nuniform float u_maskAge;\nuniform float u_textInfluence;\nuniform float u_lifeGeneration;\n`);
  const codeWithoutComments=body.replace(/\/\*[\s\S]*?\*\//g,'').replace(/\/\/[^\n]*/g,'');
  const hasMain=/\bvoid\s+main\s*\(/.test(codeWithoutComments);
  const footer=hasMain?'':`\nvoid main(){vec4 color=vec4(0.0,0.0,0.0,1.0);mainImage(color,gl_FragCoord.xy);${webgl2?'studioOutput':'gl_FragColor'}=color;}`;
  return header+'#line 1\n'+body+footer;
}
const PALETTES=['violet','mint','amber','mono'];
const clamp=(value,min,max)=>Math.max(min,Math.min(max,value));
// Build the upload and optional positive-alpha emission. This uses
// only the independent CPU glyph image; it never reads back a shader frame.
function flipMaskRows(mask){
  const flipped=new Uint8Array(mask.data.length);
  for(let y=0;y<mask.height;y++)flipped.set(mask.data.subarray(y*mask.width,(y+1)*mask.width),(mask.height-1-y)*mask.width);
  return flipped;
}
export function prepareMaskUpload(mask,previous,emit=true){
  const sameSize=!!previous&&mask.width===previous.width&&mask.height===previous.height;
  const sameEmission=Number.isFinite(mask.emissionRevision)&&Number.isFinite(previous?.emissionRevision)&&mask.emissionRevision===previous.emissionRevision;
  const mayEmit=emit&&!sameEmission;
  const flipped=flipMaskRows(mask);
  let emission=null,changed=!sameSize;
  for(let y=0;y<mask.height;y++){
    const source=y*mask.width,target=(mask.height-1-y)*mask.width;
    for(let x=0;x<mask.width;x++){
      const value=mask.data[source+x],before=sameSize?previous.data[source+x]:0;
      if(value!==before)changed=true;
      const added=mayEmit?Math.max(0,value-before):0;
      if(added){if(!emission)emission=new Uint8Array(mask.data.length);emission[target+x]=added;}
    }
  }
  return {flipped,emission,changed};
}

// Top-down straight RGBA accompanies, but never replaces, the independent R8 mask.
// Alpha is copied from the authoritative mask, including for image-only callers.
export function prepareGlyphColorUpload(mask){
  const rgba=new Uint8Array(mask.data.length*4),colors=mask.colorData;
  for(let y=0;y<mask.height;y++){
    const source=y*mask.width,target=(mask.height-1-y)*mask.width;
    for(let x=0;x<mask.width;x++){
      const alpha=mask.data[source+x],from=(source+x)*4,to=(target+x)*4;
      if(alpha){
        rgba[to]=colors?colors[from]:220;
        rgba[to+1]=colors?colors[from+1]:226;
        rgba[to+2]=colors?colors[from+2]:232;
      }
      rgba[to+3]=alpha;
    }
  }
  return rgba;
}

export class ShaderEngine{
  constructor(canvas,{onStats,onError,onStatus,onMask}={}){
    Object.assign(this,{canvas,onStats,onError,onStatus,onMask});
    this.config={speed:1,scale:.5,palette:'violet',feedback:.88,textInfluence:1};
    this.running=true;this.enabled=true;this.available=false;this.time=0;this.frame=0;this.preset='aurora';this.source=AURORA;
    this.maskRevision=0;this.maskEmissionRevision=0;this.maskWidth=1;this.maskHeight=1;this.maskFormat='unavailable';
    this._maskCPU={width:1,height:1,data:new Uint8Array(1),revision:0};this._pendingMask=this._maskCPU;this._uploadedMaskCPU=null;
    this._raf=0;this._lastTick=null;this._destroyed=false;this._lost=false;this._needsDraw=true;this._inject=0;this._fieldIndex=0;this._maskTime=this.time;
    this.lifeGeneration=0;this.lifeWidth=1;this.lifeHeight=1;this._usesLife=false;this._lifeFields=[];this._lifeIndex=0;this._lifeAccumulator=0;this._lifeNeedsSeed=true;this._lifeEmissionPending=false;this._lifeCSSWidth=0;this._lifeCSSHeight=0;
    this._mouse=[.5,.5,0,0];this._pointerDown=false;this._statsStart=performance.now();this._lastStats=this._statsStart;this._statsFrames=0;
    this._tick=this._tick.bind(this);this._onResize=()=>this.resize();
    this._onVisibility=()=>{this._stop();if(!document.hidden)this._schedule();};
    this._onPointer=event=>{const r=canvas.getBoundingClientRect();if(!r.width||!r.height)return;this._mouse[0]=clamp((event.clientX-r.left)/r.width,0,1);this._mouse[1]=clamp(1-(event.clientY-r.top)/r.height,0,1);if(event.type==='pointerdown'){this._pointerDown=true;this._mouse[2]=this._mouse[0];this._mouse[3]=this._mouse[1];}if(event.type==='pointerup'||event.type==='pointercancel')this._pointerDown=false;};
    this._onLost=event=>{event.preventDefault();this._lost=true;this.available=false;this._stop();this.onStatus?.('context-lost');this.onError?.('WebGL context lost. Text editing is still available.');};
    this._onRestored=()=>{this._lost=false;this.program=null;this.feedbackProgram=null;this.buffer=null;this.maskTexture=null;this.emptyTexture=null;this.lifeSeedProgram=null;this.lifeStepProgram=null;this.lifeEmitProgram=null;this.lifeColorSeedProgram=null;this.lifeColorStepProgram=null;this.lifeColorEmitProgram=null;this.lifeEmissionTexture=null;this.glyphColorTexture=null;this.fields=[];this._lifeFields=[];this._lifeNeedsSeed=true;this._lifeEmissionPending=false;this._lifeAccumulator=0;this.lifeGeneration=0;this._uploadedMaskCPU=null;this._pendingMask=this._maskCPU;if(this._initialize()){this.resize();this.compile(this.source);this.onStatus?.(this._status());this._schedule();}};
    canvas.addEventListener('webglcontextlost',this._onLost);canvas.addEventListener('webglcontextrestored',this._onRestored);document.addEventListener('visibilitychange',this._onVisibility);
    this._pointerTarget=canvas.parentElement||canvas;for(const e of ['pointermove','pointerdown','pointerup','pointercancel'])this._pointerTarget.addEventListener(e,this._onPointer,{passive:true});
    if(typeof ResizeObserver!=='undefined'){this._observer=new ResizeObserver(this._onResize);this._observer.observe(canvas);}else window.addEventListener('resize',this._onResize);
    if(this._initialize()){this.resize();this.compile(this.source);}
  }
  _initialize(){
    const options={alpha:false,antialias:false,depth:false,stencil:false,premultipliedAlpha:false,preserveDrawingBuffer:true,powerPreference:'low-power'};
    this.gl=this.canvas.getContext('webgl2',options);this.webgl2=!!this.gl;if(!this.gl)this.gl=this.canvas.getContext('webgl',options);
    if(!this.gl){this.onStatus?.('unavailable');this.onError?.('WebGL is unavailable in this browser. Text editing still works.');return false;}
    const gl=this.gl;this.available=true;this.maskFormat=this.webgl2?'R8 / RED':'LUMINANCE8 / R';this.precision=gl.getShaderPrecisionFormat(gl.FRAGMENT_SHADER,gl.HIGH_FLOAT)?.precision?'highp':'mediump';
    const v=this._shader(gl.VERTEX_SHADER,vertexSource(this.webgl2));if(!v.ok){this.available=false;this.onStatus?.('unavailable');this.onError?.(v.error);return false;}this.vertexShader=v.shader;
    this.buffer=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,this.buffer);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,3,-1,-1,3]),gl.STATIC_DRAW);
    gl.disable(gl.DEPTH_TEST);gl.disable(gl.BLEND);gl.pixelStorei(gl.UNPACK_ALIGNMENT,1);
    this.maskTexture=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,this.maskTexture);this._textureParameters();
    gl.texImage2D(gl.TEXTURE_2D,0,this.webgl2?gl.R8:gl.LUMINANCE,1,1,0,this.webgl2?gl.RED:gl.LUMINANCE,gl.UNSIGNED_BYTE,new Uint8Array(1));
    this.emptyTexture=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,this.emptyTexture);this._textureParameters(gl.NEAREST);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,1,1,0,gl.RGBA,gl.UNSIGNED_BYTE,new Uint8Array(4));
    const fb=this._program(FEEDBACK_GLSL,true);if(!fb.ok){this.available=false;this.onStatus?.('unavailable');this.onError?.(fb.error);return false;}
    this.feedbackProgram=fb.program;this.feedbackUniforms=Object.fromEntries(['u_previous','u_glyphs','u_resolution','u_time','u_delta','u_feedback','u_inject'].map(n=>[n,gl.getUniformLocation(fb.program,n)]));
    this.fields=[];this._allocateFields();return true;
  }
  _textureParameters(filter=this.gl.LINEAR){const gl=this.gl;gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,filter);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,filter);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_S,gl.CLAMP_TO_EDGE);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_T,gl.CLAMP_TO_EDGE);}
  _shader(type,source){const gl=this.gl,s=gl.createShader(type);gl.shaderSource(s,source);gl.compileShader(s);if(!gl.getShaderParameter(s,gl.COMPILE_STATUS)){const error=(gl.getShaderInfoLog(s)||'Shader compilation failed.').trim();gl.deleteShader(s);return{ok:false,error};}return{ok:true,shader:s};}
  _program(source,feedback=false){
    const gl=this.gl;const f=this._shader(gl.FRAGMENT_SHADER,fragmentSource(source,this.webgl2,this.precision,feedback));if(!f.ok)return f;
    const p=gl.createProgram();gl.attachShader(p,this.vertexShader);gl.attachShader(p,f.shader);gl.bindAttribLocation(p,0,'a_position');gl.linkProgram(p);gl.detachShader(p,f.shader);gl.deleteShader(f.shader);
    if(!gl.getProgramParameter(p,gl.LINK_STATUS)){const error=(gl.getProgramInfoLog(p)||'Shader linking failed.').trim();gl.deleteProgram(p);return{ok:false,error};}return{ok:true,program:p};
  }
  setPreset(id){const p=PRESETS.find(p=>p.id===id);if(!p)return{ok:false,error:'Unknown shader preset.'};const result=this.compile(p.source);if(result.ok)this.preset=id;return result;}
  compile(source){
    if(!this.available||this._lost||this._destroyed)return{ok:false,error:'WebGL is not available.'};
    if(typeof source!=='string'||!source.trim())return{ok:false,error:'Enter a fragment shader first.'};
    if(!this.webgl2&&/^\s*#version\s+300/m.test(source))return{ok:false,error:'This browser uses WebGL 1. Use mainImage with GLSL ES 1.00.'};
    const next=this._program(source);if(!next.ok){this.onError?.(next.error);return next;}
    const gl=this.gl,previous=this.program;
    const body=source.replace(/\/\*[\s\S]*?\*\//g,'').replace(/\/\/[^\n]*/g,'');
    const usesLife=/\biChannel[23]\b/.test(body);
    if(usesLife){try{this._ensureLifeResources();}catch(error){gl.deleteProgram(next.program);this.onError?.(error.message);return{ok:false,error:error.message};}}
    this._usesLife=usesLife;this.program=next.program;this.source=source;this.preset=PRESETS.find(p=>p.source===source)?.id||'custom';
    this.uniforms=Object.fromEntries(UNIFORM_NAMES.map(n=>[n,gl.getUniformLocation(this.program,n)]));if(previous)gl.deleteProgram(previous);
    this.renderOnce();this.onStatus?.(this._status());this._schedule();return{ok:true};
  }
  getSource(){return this.source;}
  setTextMask(mask){
    if(this._destroyed)return false;
    if(!mask||!Number.isInteger(mask.width)||!Number.isInteger(mask.height)||mask.width<1||mask.height<1||!(mask.data instanceof Uint8Array)||mask.data.length!==mask.width*mask.height)throw new TypeError('Text mask must be width × height Uint8Array glyph coverage.');
    if(mask.colorData!==undefined&&(!(mask.colorData instanceof Uint8Array)&&!(mask.colorData instanceof Uint8ClampedArray)||mask.colorData.length!==mask.data.length*4))throw new TypeError('Glyph colors must be width × height straight RGBA bytes.');
    this._maskCPU={width:mask.width,height:mask.height,data:mask.data,colorData:mask.colorData,revision:Number.isFinite(mask.revision)?mask.revision:this.maskRevision+1,emissionRevision:Number.isFinite(mask.emissionRevision)?mask.emissionRevision:undefined,scrollRevision:Number.isFinite(mask.scrollRevision)?mask.scrollRevision:undefined};
    this._pendingMask=this._maskCPU;this._needsDraw=true;this._schedule();return true;
  }
  _uploadPendingMask(){
    if(!this._pendingMask)return false;const mask=this._pendingMask,gl=this.gl;
    const maximum=gl.getParameter(gl.MAX_TEXTURE_SIZE);if(mask.width>maximum||mask.height>maximum){this._pendingMask=null;this.onError?.('Text mask exceeds the GPU texture size limit.');return false;}
    const previous=this._uploadedMaskCPU,explicitRevision=Number.isFinite(mask.emissionRevision);
    // Scrolling is the explicit viewport-refresh exception to additive input.
    if(Number.isFinite(mask.scrollRevision)&&Number.isFinite(previous?.scrollRevision)&&mask.scrollRevision!==previous.scrollRevision){this._lifeNeedsSeed=true;this._lifeEmissionPending=false;}
    const mayEmit=this._lifeFields.length>0&&!this._lifeNeedsSeed&&(!explicitRevision||mask.emissionRevision!==previous?.emissionRevision);
    const upload=mayEmit||!explicitRevision?prepareMaskUpload(mask,previous,mayEmit):{flipped:flipMaskRows(mask),emission:null,changed:false};
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.maskTexture);gl.pixelStorei(gl.UNPACK_ALIGNMENT,1);gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,false);
    const format=this.webgl2?gl.RED:gl.LUMINANCE;
    gl.texImage2D(gl.TEXTURE_2D,0,this.webgl2?gl.R8:gl.LUMINANCE,mask.width,mask.height,0,format,gl.UNSIGNED_BYTE,upload.flipped);
    if(upload.emission){
      gl.bindTexture(gl.TEXTURE_2D,this.lifeEmissionTexture);
      gl.texImage2D(gl.TEXTURE_2D,0,this.webgl2?gl.R8:gl.LUMINANCE,mask.width,mask.height,0,format,gl.UNSIGNED_BYTE,upload.emission);
      this._lifeEmissionPending=true;
    }
    if(this.glyphColorTexture)this._uploadGlyphColors(mask);
    this.maskEmissionRevision=Number.isFinite(mask.emissionRevision)?mask.emissionRevision:this.maskEmissionRevision+(upload.changed?1:0);
    // Always advance the baseline, even when a whitespace-only snapshot emits nothing.
    this._uploadedMaskCPU=mask;
    this.maskWidth=mask.width;this.maskHeight=mask.height;this.maskRevision=mask.revision;this._maskTime=this.time;this._pendingMask=null;this._inject=1;
    this.onMask?.({revision:this.maskRevision,emissionRevision:this.maskEmissionRevision,width:this.maskWidth,height:this.maskHeight,format:this.maskFormat,bytes:mask.data.length});return true;
  }
  _uploadGlyphColors(mask){
    const gl=this.gl;
    // Only engines with retained Life resources allocate or upload this texture.
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.glyphColorTexture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT,1);gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,false);gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL,false);
    gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,mask.width,mask.height,0,gl.RGBA,gl.UNSIGNED_BYTE,prepareGlyphColorUpload(mask));
  }
  _allocateFields(){
    const gl=this.gl;if(!gl||this._lost)return;
    for(const f of this.fields||[]){gl.deleteFramebuffer(f.framebuffer);gl.deleteTexture(f.texture);}this.fields=[];
    for(let i=0;i<2;i++){const texture=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,texture);this._textureParameters();gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,this.canvas.width,this.canvas.height,0,gl.RGBA,gl.UNSIGNED_BYTE,null);const framebuffer=gl.createFramebuffer();gl.bindFramebuffer(gl.FRAMEBUFFER,framebuffer);gl.framebufferTexture2D(gl.FRAMEBUFFER,gl.COLOR_ATTACHMENT0,gl.TEXTURE_2D,texture,0);if(gl.checkFramebufferStatus(gl.FRAMEBUFFER)!==gl.FRAMEBUFFER_COMPLETE)throw new Error('The GPU feedback surface is incomplete.');gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT);this.fields.push({texture,framebuffer});}
    this._fieldIndex=0;gl.bindFramebuffer(gl.FRAMEBUFFER,null);this._inject=1;
  }
  clearFeedback(){if(!this.available||this._lost||this._destroyed)return;const gl=this.gl;for(const f of this.fields){gl.bindFramebuffer(gl.FRAMEBUFFER,f.framebuffer);gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT);}gl.bindFramebuffer(gl.FRAMEBUFFER,null);this._fieldIndex=0;this._inject=1;this._needsDraw=true;this._schedule();}
  _ensureLifeResources(){
    const gl=this.gl;
    if(!this.lifeSeedProgram){
      const programs=[];
      try{for(const source of [LIFE_SEED_GLSL,LIFE_STEP_GLSL,LIFE_EMIT_GLSL,LIFE_COLOR_SEED_GLSL,LIFE_COLOR_STEP_GLSL,LIFE_COLOR_EMIT_GLSL]){
        const result=this._program(source,true);if(!result.ok)throw new Error(result.error);programs.push(result.program);
      }}catch(error){for(const program of programs)gl.deleteProgram(program);throw error;}
      [this.lifeSeedProgram,this.lifeStepProgram,this.lifeEmitProgram,this.lifeColorSeedProgram,this.lifeColorStepProgram,this.lifeColorEmitProgram]=programs;
      const uniforms=(program,names)=>Object.fromEntries(names.map(n=>[n,gl.getUniformLocation(program,n)]));
      this.lifeSeedUniforms=uniforms(this.lifeSeedProgram,['u_glyphs','u_resolution']);
      this.lifeStepUniforms=uniforms(this.lifeStepProgram,['u_previous','u_resolution','u_trailDecay']);
      this.lifeEmitUniforms=uniforms(this.lifeEmitProgram,['u_previous','u_emission','u_resolution']);
      this.lifeColorSeedUniforms=uniforms(this.lifeColorSeedProgram,['u_glyphColors','u_state','u_resolution','u_glyphResolution']);
      this.lifeColorStepUniforms=uniforms(this.lifeColorStepProgram,['u_previous','u_next','u_previousColors','u_resolution']);
      this.lifeColorEmitUniforms=uniforms(this.lifeColorEmitProgram,['u_previous','u_previousColors','u_emission','u_glyphColors','u_resolution','u_glyphResolution']);
    }
    if(!this.lifeEmissionTexture){
      this.lifeEmissionTexture=gl.createTexture();if(!this.lifeEmissionTexture)throw new Error('The Conway emission surface could not be allocated.');
      gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.lifeEmissionTexture);this._textureParameters();
      gl.texImage2D(gl.TEXTURE_2D,0,this.webgl2?gl.R8:gl.LUMINANCE,1,1,0,this.webgl2?gl.RED:gl.LUMINANCE,gl.UNSIGNED_BYTE,new Uint8Array(1));
    }
    if(!this.glyphColorTexture){
      this.glyphColorTexture=gl.createTexture();if(!this.glyphColorTexture)throw new Error('The glyph color surface could not be allocated.');
      gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.glyphColorTexture);this._textureParameters(gl.NEAREST);
      this._uploadGlyphColors(this._uploadedMaskCPU||this._maskCPU);
    }
    return this._resizeLifeGrid();
  }
  _resizeLifeGrid(){
    const gl=this.gl,r=this.canvas.getBoundingClientRect();
    // A CSS-sized world is independent of the presentation drawing-buffer scale.
    const width=clamp(Math.ceil(r.width/3),1,512),height=clamp(Math.ceil(r.height/3),1,512);
    const geometryChanged=Math.abs(r.width-this._lifeCSSWidth)>.5||Math.abs(r.height-this._lifeCSSHeight)>.5;
    const changed=!this._lifeFields.length||width!==this.lifeWidth||height!==this.lifeHeight;
    if(changed){
      const next=[];
      const dispose=field=>{for(const name of ['framebuffer','colorFramebuffer'])if(field[name])gl.deleteFramebuffer(field[name]);for(const name of ['texture','colorTexture'])if(field[name])gl.deleteTexture(field[name]);};
      try{for(let i=0;i<2;i++){
        const field={};next.push(field);
        for(const [textureName,framebufferName] of [['texture','framebuffer'],['colorTexture','colorFramebuffer']]){
          const texture=field[textureName]=gl.createTexture(),framebuffer=field[framebufferName]=gl.createFramebuffer();
          if(!texture||!framebuffer)throw new Error('The Conway state/color surface could not be allocated.');
          gl.bindTexture(gl.TEXTURE_2D,texture);this._textureParameters(gl.NEAREST);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,width,height,0,gl.RGBA,gl.UNSIGNED_BYTE,null);
          gl.bindFramebuffer(gl.FRAMEBUFFER,framebuffer);gl.framebufferTexture2D(gl.FRAMEBUFFER,gl.COLOR_ATTACHMENT0,gl.TEXTURE_2D,texture,0);
          if(gl.checkFramebufferStatus(gl.FRAMEBUFFER)!==gl.FRAMEBUFFER_COMPLETE)throw new Error('The Conway state/color surface is incomplete.');
          gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT);
        }
      }}catch(error){for(const field of next)dispose(field);gl.bindFramebuffer(gl.FRAMEBUFFER,null);throw error;}
      if(this._lifeFields.length){
        // Copy state and color together, with no interpolation or CPU readback.
        const copyWidth=Math.min(width,this.lifeWidth),copyHeight=Math.min(height,this.lifeHeight),previous=this._lifeFields[this._lifeIndex];
        for(const [textureName,framebufferName] of [['texture','framebuffer'],['colorTexture','colorFramebuffer']]){
          if(!previous[framebufferName])continue;
          gl.bindFramebuffer(gl.FRAMEBUFFER,previous[framebufferName]);
          gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,next[0][textureName]);
          gl.copyTexSubImage2D(gl.TEXTURE_2D,0,0,height-copyHeight,0,this.lifeHeight-copyHeight,copyWidth,copyHeight);
        }
      }else this._lifeNeedsSeed=true;
      for(const field of this._lifeFields)dispose(field);
      this._lifeFields=next;this.lifeWidth=width;this.lifeHeight=height;
      this._lifeIndex=0;gl.bindFramebuffer(gl.FRAMEBUFFER,null);
    }
    this._lifeCSSWidth=r.width;this._lifeCSSHeight=r.height;
    return changed||geometryChanged;
  }
  reseedLife(){
    if(!this.available||this._lost||this._destroyed||!this._usesLife)return false;
    this._lifeNeedsSeed=true;this._lifeAccumulator=0;this._needsDraw=true;this._schedule();return true;
  }
  _seedLife(){
    const gl=this.gl,u=this.lifeSeedUniforms,write=this._lifeFields[0];
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.framebuffer);gl.viewport(0,0,this.lifeWidth,this.lifeHeight);gl.useProgram(this.lifeSeedProgram);this._bindGeometry();
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.maskTexture);gl.uniform1i(u.u_glyphs,0);gl.uniform2f(u.u_resolution,this.lifeWidth,this.lifeHeight);gl.drawArrays(gl.TRIANGLES,0,3);
    const c=this.lifeColorSeedUniforms;
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.colorFramebuffer);gl.useProgram(this.lifeColorSeedProgram);
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.glyphColorTexture);gl.uniform1i(c.u_glyphColors,0);
    gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,write.texture);gl.uniform1i(c.u_state,1);
    gl.uniform2f(c.u_resolution,this.lifeWidth,this.lifeHeight);gl.uniform2f(c.u_glyphResolution,this.maskWidth,this.maskHeight);gl.drawArrays(gl.TRIANGLES,0,3);
    // Commit one shared index only when both outputs have been drawn.
    this._lifeIndex=0;this._lifeNeedsSeed=false;this._lifeEmissionPending=false;this._lifeAccumulator=0;this.lifeGeneration=0;
    gl.bindFramebuffer(gl.FRAMEBUFFER,null);
  }
  _emitLife(){
    const gl=this.gl,u=this.lifeEmitUniforms,read=this._lifeFields[this._lifeIndex],write=this._lifeFields[1-this._lifeIndex];
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.framebuffer);gl.viewport(0,0,this.lifeWidth,this.lifeHeight);gl.useProgram(this.lifeEmitProgram);this._bindGeometry();
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,read.texture);gl.uniform1i(u.u_previous,0);
    gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,this.lifeEmissionTexture);gl.uniform1i(u.u_emission,1);gl.uniform2f(u.u_resolution,this.lifeWidth,this.lifeHeight);gl.drawArrays(gl.TRIANGLES,0,3);
    const c=this.lifeColorEmitUniforms;
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.colorFramebuffer);gl.useProgram(this.lifeColorEmitProgram);
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,read.texture);gl.uniform1i(c.u_previous,0);
    gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,read.colorTexture);gl.uniform1i(c.u_previousColors,1);
    gl.activeTexture(gl.TEXTURE2);gl.bindTexture(gl.TEXTURE_2D,this.lifeEmissionTexture);gl.uniform1i(c.u_emission,2);
    gl.activeTexture(gl.TEXTURE3);gl.bindTexture(gl.TEXTURE_2D,this.glyphColorTexture);gl.uniform1i(c.u_glyphColors,3);
    gl.uniform2f(c.u_resolution,this.lifeWidth,this.lifeHeight);gl.uniform2f(c.u_glyphResolution,this.maskWidth,this.maskHeight);gl.drawArrays(gl.TRIANGLES,0,3);
    this._lifeIndex=1-this._lifeIndex;this._lifeEmissionPending=false;gl.bindFramebuffer(gl.FRAMEBUFFER,null);
  }
  _stepLife(){
    const gl=this.gl,u=this.lifeStepUniforms,read=this._lifeFields[this._lifeIndex],write=this._lifeFields[1-this._lifeIndex];
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.framebuffer);gl.viewport(0,0,this.lifeWidth,this.lifeHeight);gl.useProgram(this.lifeStepProgram);this._bindGeometry();
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,read.texture);gl.uniform1i(u.u_previous,0);gl.uniform2f(u.u_resolution,this.lifeWidth,this.lifeHeight);gl.uniform1f(u.u_trailDecay,.68+.30*this.config.feedback);gl.drawArrays(gl.TRIANGLES,0,3);
    const c=this.lifeColorStepUniforms;
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.colorFramebuffer);gl.useProgram(this.lifeColorStepProgram);
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,read.texture);gl.uniform1i(c.u_previous,0);
    gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,write.texture);gl.uniform1i(c.u_next,1);
    gl.activeTexture(gl.TEXTURE2);gl.bindTexture(gl.TEXTURE_2D,read.colorTexture);gl.uniform1i(c.u_previousColors,2);
    gl.uniform2f(c.u_resolution,this.lifeWidth,this.lifeHeight);gl.drawArrays(gl.TRIANGLES,0,3);
    this._lifeIndex=1-this._lifeIndex;this.lifeGeneration++;gl.bindFramebuffer(gl.FRAMEBUFFER,null);
  }
  _advanceLife(delta){
    if(!this._lifeFields.length)return;
    if(this._lifeNeedsSeed){if(this._usesLife)this._seedLife();return;}
    // A retained but unselected world can receive new ink without evolving.
    if(this._lifeEmissionPending)this._emitLife();
    if(!this._usesLife)return;
    this._lifeAccumulator+=Math.max(0,delta);
    const steps=Math.min(5,Math.floor((this._lifeAccumulator+1e-9)/.1));
    for(let i=0;i<steps;i++)this._stepLife();
    this._lifeAccumulator-=steps*.1;
    // Bound a pathological catch-up backlog; normal RAF deltas never reach this.
    this._lifeAccumulator=Math.min(.5,Math.max(0,this._lifeAccumulator));
  }
  setConfig(next={}){
    if(Number.isFinite(next.speed)){const speed=clamp(next.speed,0,2);if(speed!==this.config.speed)this._stop();this.config.speed=speed;}
    if(PALETTES.includes(next.palette))this.config.palette=next.palette;
    if(Number.isFinite(next.feedback))this.config.feedback=clamp(next.feedback,0,1);
    if(Number.isFinite(next.textInfluence))this.config.textInfluence=clamp(next.textInfluence,0,1);
    let resized=false;if([.25,.5,1].includes(next.scale)&&next.scale!==this.config.scale){this.config.scale=next.scale;resized=this.resize();}if(!resized)this.renderOnce();this._schedule();
  }
  setRunning(value){this.running=Boolean(value);this._stop();this.onStatus?.(this._status());this._schedule();}
  setEnabled(value){this.enabled=Boolean(value);this._stop();if(this.enabled)this.renderOnce();this.onStatus?.(this._status());this._schedule();}
  resize(){
    if(this._destroyed)return false;const r=this.canvas.getBoundingClientRect(),dpr=Math.min(window.devicePixelRatio||1,2);const width=Math.max(1,Math.round(r.width*dpr*this.config.scale)),height=Math.max(1,Math.round(r.height*dpr*this.config.scale));
    const changed=this.canvas.width!==width||this.canvas.height!==height;
    if(changed){this.canvas.width=width;this.canvas.height=height;if(this.gl&&!this._lost){this.gl.viewport(0,0,width,height);this._allocateFields();}}
    const lifeChanged=(this._usesLife||this._lifeFields.length)&&this.available&&!this._lost?this._ensureLifeResources():false;
    if(!changed&&!lifeChanged)return false;this.renderOnce();return true;
  }
  renderOnce(){if(!this.enabled||!this.available||this._lost||this._destroyed||!this.program)return;this._draw(0);this._needsDraw=false;}
  _bindGeometry(){const gl=this.gl;gl.bindBuffer(gl.ARRAY_BUFFER,this.buffer);gl.enableVertexAttribArray(0);gl.vertexAttribPointer(0,2,gl.FLOAT,false,0,0);}
  _advanceField(delta){
    if(delta===0&&!this._inject)return;const gl=this.gl,read=this.fields[this._fieldIndex],write=this.fields[1-this._fieldIndex],u=this.feedbackUniforms;
    gl.bindFramebuffer(gl.FRAMEBUFFER,write.framebuffer);gl.viewport(0,0,this.canvas.width,this.canvas.height);gl.useProgram(this.feedbackProgram);this._bindGeometry();
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,read.texture);gl.uniform1i(u.u_previous,0);gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,this.maskTexture);gl.uniform1i(u.u_glyphs,1);
    gl.uniform2f(u.u_resolution,this.canvas.width,this.canvas.height);gl.uniform1f(u.u_time,this.time);gl.uniform1f(u.u_delta,delta);gl.uniform1f(u.u_feedback,this.config.feedback);gl.uniform1f(u.u_inject,this._inject);gl.drawArrays(gl.TRIANGLES,0,3);
    this._fieldIndex=1-this._fieldIndex;this._inject=0;gl.bindFramebuffer(gl.FRAMEBUFFER,null);
  }
  _draw(delta){
    const gl=this.gl;this._uploadPendingMask();this._advanceField(delta);this._advanceLife(delta);gl.bindFramebuffer(gl.FRAMEBUFFER,null);gl.viewport(0,0,this.canvas.width,this.canvas.height);gl.useProgram(this.program);this._bindGeometry();const u=this.uniforms;
    gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,this.maskTexture);gl.uniform1i(u.iChannel0,0);gl.activeTexture(gl.TEXTURE1);gl.bindTexture(gl.TEXTURE_2D,this.fields[this._fieldIndex].texture);gl.uniform1i(u.iChannel1,1);
    gl.activeTexture(gl.TEXTURE2);gl.bindTexture(gl.TEXTURE_2D,this._usesLife?this._lifeFields[this._lifeIndex].texture:this.emptyTexture);gl.uniform1i(u.iChannel2,2);
    gl.activeTexture(gl.TEXTURE3);gl.bindTexture(gl.TEXTURE_2D,this._usesLife?this._lifeFields[this._lifeIndex].colorTexture:this.emptyTexture);gl.uniform1i(u.iChannel3,3);
    gl.uniform3f(u.iResolution,this.canvas.width,this.canvas.height,1);gl.uniform1f(u.iTime,this.time);gl.uniform1f(u.iFrame,this.frame);gl.uniform1f(u.u_palette,PALETTES.indexOf(this.config.palette));gl.uniform1f(u.u_maskRevision,this.maskRevision);gl.uniform1f(u.u_emissionRevision,this.maskEmissionRevision);gl.uniform1f(u.u_maskAge,Math.max(0,this.time-this._maskTime));gl.uniform1f(u.u_textInfluence,this.config.textInfluence);gl.uniform1f(u.u_lifeGeneration,this.lifeGeneration);
    gl.uniform3f(u['iChannelResolution[0]'],this.maskWidth,this.maskHeight,1);
    gl.uniform3f(u['iChannelResolution[1]'],this.canvas.width,this.canvas.height,1);
    gl.uniform3f(u['iChannelResolution[2]'],this._usesLife?this.lifeWidth:1,this._usesLife?this.lifeHeight:1,1);
    gl.uniform3f(u['iChannelResolution[3]'],this._usesLife?this.lifeWidth:1,this._usesLife?this.lifeHeight:1,1);
    const sign=this._pointerDown?1:-1;gl.uniform4f(u.iMouse,this._mouse[0]*this.canvas.width,this._mouse[1]*this.canvas.height,sign*this._mouse[2]*this.canvas.width,sign*this._mouse[3]*this.canvas.height);gl.drawArrays(gl.TRIANGLES,0,3);this.frame++;this._statsFrames++;
  }
  _tick(now){
    this._raf=0;if(!this.enabled||!this.available||this._lost||this._destroyed||document.hidden){this._lastTick=null;return;}
    const animating=this.running&&this.config.speed>0;const delta=animating&&this._lastTick!==null?Math.min((now-this._lastTick)/1000,.05)*this.config.speed:0;this._lastTick=animating?now:null;this.time+=delta;
    if(animating||this._needsDraw||this._pendingMask){this._draw(delta);this._needsDraw=false;}
    if(now-this._lastStats>=1000){this.onStats?.({fps:this._statsFrames/Math.max(.001,(now-this._statsStart)/1000),width:this.canvas.width,height:this.canvas.height,time:this.time,frame:this.frame,maskRevision:this.maskRevision,emissionRevision:this.maskEmissionRevision,maskFormat:this.maskFormat,lifeGeneration:this.lifeGeneration,lifeWidth:this.lifeWidth,lifeHeight:this.lifeHeight});this._lastStats=now;this._statsStart=now;this._statsFrames=0;}
    this._schedule();
  }
  _schedule(){if(!this._raf&&this.enabled&&this.available&&this.program&&!this._lost&&!this._destroyed&&!document.hidden&&((this.running&&this.config.speed>0)||this._needsDraw||this._pendingMask))this._raf=requestAnimationFrame(this._tick);}
  _status(){if(this._lost)return'context-lost';if(!this.available)return'unavailable';if(!this.enabled)return'disabled';return this.running?'running':'paused';}
  _stop(){cancelAnimationFrame(this._raf);this._raf=0;this._lastTick=null;this._statsStart=performance.now();this._lastStats=this._statsStart;this._statsFrames=0;}
  destroy(){
    this._destroyed=true;this._stop();this._observer?.disconnect();window.removeEventListener('resize',this._onResize);document.removeEventListener('visibilitychange',this._onVisibility);this.canvas.removeEventListener('webglcontextlost',this._onLost);this.canvas.removeEventListener('webglcontextrestored',this._onRestored);for(const e of ['pointermove','pointerdown','pointerup','pointercancel'])this._pointerTarget.removeEventListener(e,this._onPointer);
    if(this.gl&&!this._lost){const gl=this.gl;if(this.program)gl.deleteProgram(this.program);if(this.feedbackProgram)gl.deleteProgram(this.feedbackProgram);if(this.lifeSeedProgram)gl.deleteProgram(this.lifeSeedProgram);if(this.lifeStepProgram)gl.deleteProgram(this.lifeStepProgram);if(this.lifeEmitProgram)gl.deleteProgram(this.lifeEmitProgram);if(this.lifeEmissionTexture)gl.deleteTexture(this.lifeEmissionTexture);if(this.glyphColorTexture)gl.deleteTexture(this.glyphColorTexture);for(const program of [this.lifeColorSeedProgram,this.lifeColorStepProgram,this.lifeColorEmitProgram])if(program)gl.deleteProgram(program);if(this.vertexShader)gl.deleteShader(this.vertexShader);if(this.buffer)gl.deleteBuffer(this.buffer);if(this.maskTexture)gl.deleteTexture(this.maskTexture);if(this.emptyTexture)gl.deleteTexture(this.emptyTexture);for(const f of [...(this.fields||[]),...(this._lifeFields||[])]){gl.deleteFramebuffer(f.framebuffer);gl.deleteTexture(f.texture);if(f.colorFramebuffer)gl.deleteFramebuffer(f.colorFramebuffer);if(f.colorTexture)gl.deleteTexture(f.colorTexture);}}
  }
}

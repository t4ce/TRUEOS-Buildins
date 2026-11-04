import assert from 'node:assert/strict';
import test from 'node:test';
import {ShaderEngine,PRESETS,prepareGlyphColorUpload,UNIFORM_NAMES,fragmentSource,LIFE_GLSL,FEEDBACK_GLSL,LIFE_SEED_GLSL,LIFE_STEP_GLSL,LIFE_EMIT_GLSL} from '../dist/shader-engine.js';
import {createHash} from 'node:crypto';
const sourceHash=value=>createHash('sha256').update(value).digest('hex');
const unchangedV6Hashes={
  "aurora": "56c2cc58a77fcf6b051e739081646a5829df7167de1fa76871b2e184878b5ef8",
  "wave": "7ffd8e64960e7af52a020a58a004be1d04a2e679be720b7e8e703a8a491f3505",
  "cubes": "d05364ad88fffd9d8b42be9b4e920f1a6196fd7dc87412c807768aa284fa793b",
  "contour": "262e08c9d374d077a381be87f15f9db844e63b12842e6be02d20d3a24636f2d8",
  "dither": "385cafdb819fdc9b0262e711d6b1dbab7052eaece7dead25e5d725e7b7b9d916",
  "echo": "99af17c8e7ec0611222924759fc34f3db9ebd3cc872fa73af10d8a9079faaaaf",
  "matrix": "78a18a6556117be77a0b1283aa3f5418880244bfe0e1367f25cab2b85b753568",
  "FEEDBACK_GLSL": "7f2b68c12dd5c8810df9b0e6b0295d5cbf908fcbef23b644f41d1a94d4ca8964",
  "LIFE_SEED_GLSL": "2247f36e19962558c875d7424187e4517a21eb7f0730e52755fc6dde2da2206e",
  "LIFE_STEP_GLSL": "6f9b34ab7db227f69d9de1fdf9eb2ea1f40a4c46ab1a28a7336c87ed8f009dbb",
  "LIFE_EMIT_GLSL": "441a5980e09ef7b1ad574784acf708612e43d059d578e862649494de133937e9"
};

const mask=(alpha=[255,0,128,0,80,255],color=[12,199,71],emissionRevision=2)=>({width:3,height:2,data:new Uint8Array(alpha),colorData:new Uint8Array(alpha.flatMap(a=>[...color,a])),revision:2,emissionRevision,scrollRevision:0});
function fakeGL(){
 let id=0;const state={uploads:[],draws:[],copies:[],deletedTextures:[],deletedFramebuffers:[],deletedPrograms:[],bindings:{},active:'TEXTURE0',framebuffer:null,program:null};
 const gl=new Proxy({...state,
  getParameter:()=>4096,getShaderPrecisionFormat:()=>({precision:23}),getShaderParameter:()=>true,getProgramParameter:()=>true,
  createTexture:()=>({type:'texture',id:++id}),createFramebuffer:()=>({type:'framebuffer',id:++id}),createProgram:()=>({type:'program',id:++id}),createBuffer:()=>({type:'buffer',id:++id}),createShader:()=>({type:'shader',id:++id}),
  activeTexture(unit){this.active=unit;},bindTexture(_,texture){this.bindings[this.active]=texture;},bindFramebuffer(_,framebuffer){this.framebuffer=framebuffer;},useProgram(program){this.program=program;},
  texImage2D(...args){this.uploads.push({texture:this.bindings[this.active],args});},
  copyTexSubImage2D(...args){this.copies.push({source:this.framebuffer,target:this.bindings[this.active],args});},
  drawArrays(){this.draws.push({program:this.program,target:this.framebuffer,inputs:{...this.bindings},index:this.engine?._lifeIndex,generation:this.engine?.lifeGeneration});},
  deleteTexture(value){this.deletedTextures.push(value);},deleteFramebuffer(value){this.deletedFramebuffers.push(value);},deleteProgram(value){this.deletedPrograms.push(value);},
  checkFramebufferStatus:()=> 'FRAMEBUFFER_COMPLETE',getUniformLocation:(_,name)=>name,
 },{get(target,key){if(key in target)return target[key];if(/^[A-Z0-9_]+$/.test(key))return key;return ()=>{};}});
 return gl;
}
function engine(){
 const e=Object.create(ShaderEngine.prototype),gl=fakeGL();gl.engine=e;
 Object.assign(e,{gl,webgl2:true,available:true,enabled:true,_lost:false,_destroyed:false,_usesLife:true,_lifeFields:[],_lifeNeedsSeed:true,_lifeEmissionPending:false,_lifeIndex:0,_lifeAccumulator:.07,lifeGeneration:42,lifeWidth:1,lifeHeight:1,_lifeCSSWidth:0,_lifeCSSHeight:0,maskWidth:3,maskHeight:2,maskRevision:1,maskEmissionRevision:1,time:2.5,maskTexture:{name:'r8'},_maskCPU:mask(),_pendingMask:null,_uploadedMaskCPU:null,config:{feedback:.88},canvas:{getBoundingClientRect:()=>({width:12,height:9})},_schedule(){},_stop(){},_bindGeometry(){},renderOnce(){},_program:()=>({ok:true,program:gl.createProgram()})});
 return e;
}

test('RGBA upload flips rows, preserves straight source RGB and black, and uses authoritative R8 alpha',()=>{
 const source=mask([255,0,128,0,80,255]);source.colorData.set([0,0,0,7],8);source.colorData[3]=3;
 const pixels=prepareGlyphColorUpload(source);
 assert.deepEqual([...pixels],[0,0,0,0,12,199,71,80,12,199,71,255,12,199,71,255,0,0,0,0,0,0,0,128]);
 const fallback=prepareGlyphColorUpload({...source,colorData:undefined});assert.deepEqual([...fallback.slice(12,16)],[220,226,232,255]);
});

test('Color bytes stay a CPU reference until the engine has Life resources',()=>{
 const e=engine(),source=mask();e._usesLife=false;e._lifeFields=[];e._lifeNeedsSeed=true;
 e.setTextMask(source);assert.equal(e._pendingMask.colorData,source.colorData);e._uploadPendingMask();
 assert.equal(e.gl.uploads.length,1);assert.equal(e.glyphColorTexture,undefined);
 e._ensureLifeResources();assert.ok(e.glyphColorTexture);assert.equal(e.gl.uploads.find(u=>u.texture===e.glyphColorTexture).args[3],3);
 assert.deepEqual([...e.gl.uploads.find(u=>u.texture===e.glyphColorTexture).args[8]], [...prepareGlyphColorUpload(source)]);
});

test('RGB-only captures update source color without emitting or recoloring the retained world',()=>{
 const e=engine();e._ensureLifeResources();e._lifeNeedsSeed=false;e._uploadedMaskCPU=mask();e._pendingMask=mask(undefined,[251,9,203]);
 const fields=e._lifeFields,index=e._lifeIndex;e.gl.uploads=[];e._uploadPendingMask();e._advanceLife(0);
 assert.equal(e.gl.uploads.length,2);assert.equal(e._lifeEmissionPending,false);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);assert.equal(e._lifeFields,fields);assert.equal(e._lifeIndex,index);assert.equal(e.gl.draws.length,0);
});

test('Positive alpha delta emits once while uploading the complete new source RGB',()=>{
 const e=engine();e._ensureLifeResources();e._lifeNeedsSeed=false;e._uploadedMaskCPU=mask([20,0,128,0,80,255],[255,1,7],1);e._pendingMask=mask([100,0,128,0,80,255],[12,199,71],2);
 e.gl.uploads=[];e._uploadPendingMask();assert.equal(e.gl.uploads.length,3);assert.equal(e._lifeEmissionPending,true);
 const source=e.gl.uploads.find(u=>u.texture===e.glyphColorTexture).args[8];assert.deepEqual([...source.slice(12,16)],[12,199,71,100]);
 e._advanceLife(0);assert.equal(e._lifeEmissionPending,false);assert.equal(e.lifeGeneration,42);assert.equal(e.gl.draws.length,2);e._advanceLife(0);assert.equal(e.gl.draws.length,2);
});

test('Color input validation accepts straight byte arrays and rejects malformed dimensions',()=>{
 const e=engine();assert.throws(()=>e.setTextMask({...mask(),colorData:new Uint8Array(3)}),TypeError);assert.throws(()=>e.setTextMask({...mask(),colorData:null}),TypeError);
 const clamped=new Uint8ClampedArray(mask().colorData);e.setTextMask({...mask(),colorData:clamped});assert.equal(e._maskCPU.colorData,clamped);
});

test('Either Life channel activates resources and the wrapper exposes exactly four channels',()=>{
 const e=engine();e.program={};const onlyColor='void mainImage(out vec4 c,in vec2 p){c=texture2D(iChannel3,p/iResolution.xy);}';
 assert.equal(e.compile(onlyColor).ok,true);assert.equal(e._usesLife,true);assert.ok(e._lifeFields[0].colorTexture);assert.equal(e.lifeGeneration,42);
 const fields=e._lifeFields;assert.equal(e.compile(PRESETS[0].source).ok,true);assert.equal(e._usesLife,false);assert.equal(e._lifeFields,fields);
 assert.ok(UNIFORM_NAMES.includes('iChannelResolution[3]'));assert.match(fragmentSource(onlyColor),/iChannelResolution\[4\]/);assert.doesNotMatch(fragmentSource(onlyColor),/sampler2D iChannel4/);
});

test('State and color emission read the same old pair and commit the shared index after both draws',()=>{
 const e=engine();e._ensureLifeResources();e._lifeNeedsSeed=false;e._lifeEmissionPending=true;const read=e._lifeFields[0],write=e._lifeFields[1];
 e._emitLife();const [state,color]=e.gl.draws;
 assert.equal(state.target,write.framebuffer);assert.equal(color.target,write.colorFramebuffer);assert.equal(state.inputs.TEXTURE0,read.texture);
 assert.equal(color.inputs.TEXTURE0,read.texture);assert.equal(color.inputs.TEXTURE1,read.colorTexture);assert.equal(color.inputs.TEXTURE3,e.glyphColorTexture);
 assert.equal(state.index,0);assert.equal(color.index,0);assert.equal(e._lifeIndex,1);assert.equal(e.lifeGeneration,42);
});

test('Generation pass uses new birth flags plus old RGB and advances only after both outputs',()=>{
 const e=engine();e._ensureLifeResources();const read=e._lifeFields[0],write=e._lifeFields[1];e._stepLife();const [state,color]=e.gl.draws;
 assert.equal(state.target,write.framebuffer);assert.equal(color.target,write.colorFramebuffer);assert.equal(color.inputs.TEXTURE0,read.texture);assert.equal(color.inputs.TEXTURE1,write.texture);assert.equal(color.inputs.TEXTURE2,read.colorTexture);
 assert.equal(state.index,0);assert.equal(color.index,0);assert.equal(state.generation,42);assert.equal(color.generation,42);assert.equal(e.lifeGeneration,43);assert.equal(e._lifeIndex,1);
});

test('Manual restart and scroll refresh reseed both fields and preserve the animation clock',()=>{
 const e=engine();e._ensureLifeResources();e._lifeNeedsSeed=false;e._lifeIndex=1;e._uploadedMaskCPU=mask();e.reseedLife();e._advanceLife(0);
 assert.equal(e.gl.draws.length,2);assert.equal(e.gl.draws[0].target,e._lifeFields[0].framebuffer);assert.equal(e.gl.draws[1].target,e._lifeFields[0].colorFramebuffer);assert.equal(e.lifeGeneration,0);assert.equal(e.time,2.5);
 e.lifeGeneration=20;e._pendingMask={...mask(),scrollRevision:1};e._uploadPendingMask();e._advanceLife(0);
 assert.equal(e.gl.draws.length,4);assert.equal(e.lifeGeneration,0);assert.equal(e.time,2.5);assert.equal(e._lifeEmissionPending,false);
});

test('Resize copies state and RGB from the same old index without resetting time or generation',()=>{
 const e=engine();e._ensureLifeResources();e._lifeNeedsSeed=false;e._lifeIndex=1;const old=e._lifeFields[1];e.canvas.getBoundingClientRect=()=>({width:18,height:15});
 e._resizeLifeGrid();assert.equal(e.gl.copies.length,2);assert.equal(e.gl.copies[0].source,old.framebuffer);assert.equal(e.gl.copies[1].source,old.colorFramebuffer);
 assert.equal(e.gl.copies[0].target,e._lifeFields[0].texture);assert.equal(e.gl.copies[1].target,e._lifeFields[0].colorTexture);assert.deepEqual(e.gl.copies[0].args,e.gl.copies[1].args);
 assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);assert.equal(e._lifeNeedsSeed,false);assert.equal(e.time,2.5);
});

test('A failed color framebuffer allocation leaves the complete old pair usable',()=>{
 const e=engine();e._ensureLifeResources();const old=e._lifeFields;e.canvas.getBoundingClientRect=()=>({width:18,height:15});let checks=0;
 e.gl.checkFramebufferStatus=()=>++checks===2?'INCOMPLETE':'FRAMEBUFFER_COMPLETE';
 assert.throws(()=>e._resizeLifeGrid(),/incomplete/);assert.equal(e._lifeFields,old);assert.equal(e.lifeWidth,4);assert.equal(e.gl.deletedTextures.filter(t=>old.some(f=>f.texture===t||f.colorTexture===t)).length,0);
});

test('Seven non-Life displays, generic feedback, and all occupancy shader bodies remain byte-identical',()=>{
 for(const p of PRESETS.filter(p=>p.id!=='life'))assert.equal(sourceHash(p.source),unchangedV6Hashes[p.id],p.id);
 for(const [name,value] of Object.entries({FEEDBACK_GLSL,LIFE_SEED_GLSL,LIFE_STEP_GLSL,LIFE_EMIT_GLSL}))assert.equal(sourceHash(value),unchangedV6Hashes[name],name);
 assert.match(LIFE_GLSL,/iChannel3/);
});

test('Real engine context restoration rebuilds color resources from retained CPU input and destroy releases them',()=>{
 const listeners={};globalThis.document={hidden:false,addEventListener(){},removeEventListener(){}};globalThis.window={devicePixelRatio:1,addEventListener(){},removeEventListener(){}};globalThis.requestAnimationFrame=()=>1;globalThis.cancelAnimationFrame=()=>{};
 const gl=fakeGL(),canvas={width:12,height:9,getContext:()=>gl,getBoundingClientRect:()=>({width:12,height:9}),addEventListener(name,fn){listeners[name]=fn;},removeEventListener(){}};
 const e=new ShaderEngine(canvas);gl.engine=e;assert.equal(e.glyphColorTexture,undefined);e.setTextMask(mask());e.setPreset('life');const old=e.glyphColorTexture,retained=e._maskCPU.colorData;e.lifeGeneration=90;
 listeners.webglcontextlost({preventDefault(){}});listeners.webglcontextrestored();assert.notEqual(e.glyphColorTexture,old);assert.equal(e._maskCPU.colorData,retained);assert.equal(e.lifeGeneration,0);assert.ok(e._lifeFields.every(f=>f.colorTexture&&f.colorFramebuffer));
 const texture=e.glyphColorTexture,colors=e._lifeFields.map(f=>f.colorTexture),frames=e._lifeFields.map(f=>f.colorFramebuffer),programs=[e.lifeColorSeedProgram,e.lifeColorStepProgram,e.lifeColorEmitProgram];e.destroy();
 for(const t of [texture,...colors])assert.ok(gl.deletedTextures.includes(t));for(const f of frames)assert.ok(gl.deletedFramebuffers.includes(f));for(const p of programs)assert.ok(gl.deletedPrograms.includes(p));
});

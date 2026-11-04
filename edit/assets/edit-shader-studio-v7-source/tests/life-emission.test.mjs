import assert from 'node:assert/strict';
import test from 'node:test';
import { ShaderEngine, PRESETS, UNIFORM_NAMES, prepareMaskUpload } from '../dist/shader-engine.js';
// Replace only the browser/GPU boundary; exercise the production lifecycle.
// Actual fragment behavior and resize copies are also validated in EGL.
const check=test;
const mask=(data,emissionRevision,revision=1)=>({width:3,height:2,data:new Uint8Array(data),emissionRevision,revision});
check('Positive alpha emits new coverage only and flips rows for GL',()=>{
 const before=mask([200,0,0,0,180,0],1),after=mask([200,255,0,0,100,90],2);
 const upload=prepareMaskUpload(after,before);
 assert.deepEqual([...upload.flipped],[0,100,90,200,255,0]);
 assert.deepEqual([...upload.emission],[0,0,90,0,255,0]);
 assert.equal(upload.changed,true);
});
check('Same emission revision suppresses whitespace/layout movement even when pixels differ',()=>{
 const before=mask([255,0,0,0,0,0],7),after=mask([0,0,0,255,0,0],7,2);
 const upload=prepareMaskUpload(after,before);
 assert.equal(upload.emission,null);assert.equal(upload.changed,true);
});
check('Identical snapshots and deletions emit no live cells',()=>{
 const before=mask([255,0,128,0,0,0],1);
 assert.equal(prepareMaskUpload({...before,revision:2},before).emission,null);
 assert.equal(prepareMaskUpload(mask([0,0,0,0,0,0],2),before).emission,null);
});
check('Whitespace snapshot becomes the next baseline, preventing replay on following input',()=>{
 const moved=mask([0,0,0,255,0,0],7),typed=mask([0,0,0,255,255,0],8);
 assert.deepEqual([...prepareMaskUpload(typed,moved).emission],[0,255,0,0,0,0]);
});
function fakeGL(){
 let id=0;const uploads=[],copies=[];
 const gl=new Proxy({uploads,copies,getParameter:()=>4096,texImage2D(...a){uploads.push(a);},createTexture:()=>({id:++id}),createFramebuffer:()=>({id:++id}),checkFramebufferStatus:()=> 'FRAMEBUFFER_COMPLETE',getUniformLocation:(_,name)=>name,copyTexSubImage2D(...a){copies.push(a);}}, {get(target,key){if(key in target)return target[key];if(/^[A-Z0-9_]+$/.test(key))return key;return ()=>{};}});
 return gl;
}
function engine(){
 const e=Object.create(ShaderEngine.prototype),gl=fakeGL();
 Object.assign(e,{gl,webgl2:true,available:true,enabled:true,source:'',program:{id:100},config:{feedback:.88},_destroyed:false,_lost:false,_usesLife:true,_lifeFields:[{texture:1,framebuffer:2},{texture:3,framebuffer:4}],_lifeNeedsSeed:false,_lifeEmissionPending:false,_lifeAccumulator:.07,_lifeIndex:0,lifeWidth:4,lifeHeight:3,lifeGeneration:42,maskRevision:1,maskEmissionRevision:1,time:2.5,maskTexture:5,lifeEmissionTexture:6,_pendingMask:null,_uploadedMaskCPU:mask([255,0,0,0,0,0],1),_schedule(){},_stop(){},_bindGeometry(){},renderOnce(){},_textureParameters(){}});
 e._emitLife=function(){this.emissions=(this.emissions||0)+1;this._lifeEmissionPending=false;};
 e._stepLife=function(){this.lifeGeneration++;};
 return e;
}
check('Upload plus one emission preserves generation, time, and partial timestep',()=>{
 const e=engine();e._pendingMask=mask([255,255,0,0,0,0],2,2);
 assert.equal(e._uploadPendingMask(),true);assert.equal(e.gl.uploads.length,2);
 assert.equal(e._lifeNeedsSeed,false);assert.equal(e._lifeEmissionPending,true);assert.equal(e.maskEmissionRevision,2);
 e._advanceLife(0);
 assert.equal(e.emissions,1);assert.equal(e.lifeGeneration,42);assert.equal(e.time,2.5);assert.equal(e._lifeAccumulator,.07);
 e._advanceLife(0);assert.equal(e.emissions,1);
 e._advanceLife(.03);assert.equal(e.lifeGeneration,43);assert.ok(e._lifeAccumulator<1e-8);
});
check('Whitespace upload changes raw input without emission or reset',()=>{
 const e=engine();e._pendingMask=mask([0,0,0,255,0,0],1,2);
 e._uploadPendingMask();e._advanceLife(0);
 assert.equal(e.gl.uploads.length,1);assert.equal(e.maskRevision,2);assert.equal(e.maskEmissionRevision,1);
 assert.equal(e.emissions,undefined);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);
 assert.deepEqual([...e._uploadedMaskCPU.data],[0,0,0,255,0,0]);
});
check('Legacy image-only callers advance emission revision only for changed pixels',()=>{
 const e=engine();e._pendingMask=mask([255,0,0,0,0,0],undefined,2);
 e._uploadPendingMask();assert.equal(e.maskEmissionRevision,1);assert.equal(e._lifeEmissionPending,false);
 e._pendingMask=mask([255,128,0,0,0,0],undefined,3);e._uploadPendingMask();assert.equal(e.maskEmissionRevision,2);e._advanceLife(0);
 e._pendingMask=mask([0,0,0,0,0,0],undefined,4);e._uploadPendingMask();assert.equal(e.maskEmissionRevision,3);assert.equal(e._lifeEmissionPending,false);
});
check('Incoming metadata is retained and exposed to custom shader uniform',()=>{
 const e=engine();e.setTextMask(mask([255,0,0,0,0,0],5));assert.equal(e._pendingMask.emissionRevision,5);assert.ok(UNIFORM_NAMES.includes('u_emissionRevision'));
});
check('Reselecting Life, compiling Life, and switching away/back preserve the world',()=>{
 const e=engine();e._program=()=>({ok:true,program:{id:200}});e._ensureLifeResources=()=>false;
 assert.equal(e.setPreset('life').ok,true);assert.equal(e._lifeNeedsSeed,false);
 assert.equal(e.compile(PRESETS.find(p=>p.id==='aurora').source).ok,true);assert.equal(e._usesLife,false);
 assert.equal(e.compile(PRESETS.find(p=>p.id==='life').source).ok,true);assert.equal(e._usesLife,true);
 assert.equal(e._lifeNeedsSeed,false);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);assert.equal(e.time,2.5);
});
check('Unselected retained world receives new ink but does not advance generations',()=>{
 const e=engine();e._usesLife=false;e._lifeEmissionPending=true;e._advanceLife(.5);
 assert.equal(e.emissions,1);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);
});
check('Manual restart still replaces the world and resets its generation',()=>{
 const e=engine();e._seedLife=function(){this.seeds=(this.seeds||0)+1;this._lifeNeedsSeed=false;this._lifeEmissionPending=false;this._lifeAccumulator=0;this.lifeGeneration=0;};
 assert.equal(e.reseedLife(),true);assert.equal(e._lifeNeedsSeed,true);
 e._advanceLife(0);assert.equal(e.seeds,1);assert.equal(e.lifeGeneration,0);assert.equal(e.time,2.5);
});
check('Viewport grid resize preserves generation and copies overlap at top-left',()=>{
 const e=engine();e.canvas={getBoundingClientRect:()=>({width:18,height:15})};e._lifeCSSWidth=12;e._lifeCSSHeight=9;
 assert.equal(e._resizeLifeGrid(),true);assert.equal(e.lifeWidth,6);assert.equal(e.lifeHeight,5);
 assert.deepEqual(e.gl.copies[0],['TEXTURE_2D',0,0,2,0,0,4,3]);
 assert.equal(e._lifeNeedsSeed,false);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeAccumulator,.07);assert.equal(e.time,2.5);
});
check('CSS geometry changes within the same grid do not reset or allocate',()=>{
 const e=engine();e.canvas={getBoundingClientRect:()=>({width:11,height:8})};e._lifeCSSWidth=12;e._lifeCSSHeight=9;
 assert.equal(e._resizeLifeGrid(),true);assert.equal(e.gl.copies.length,0);assert.equal(e.lifeGeneration,42);assert.equal(e._lifeNeedsSeed,false);
});

check('A new scroll revision refreshes from the latest full mask once, without resetting the animation clock',()=>{
 const e=engine();e._uploadedMaskCPU.scrollRevision=0;e._lifeEmissionPending=true;
 e._seedLife=function(){this.seeds=(this.seeds||0)+1;this.seededRevision=this.maskRevision;this._lifeNeedsSeed=false;this._lifeEmissionPending=false;this._lifeAccumulator=0;this.lifeGeneration=0;};
 e.setTextMask({...mask([0,0,0,255,0,0],1,2),scrollRevision:1});
 e.setTextMask({...mask([0,0,0,255,255,0],2,3),scrollRevision:1});
 e._uploadPendingMask();assert.equal(e._lifeNeedsSeed,true);assert.equal(e._lifeEmissionPending,false);
 e._advanceLife(0);assert.equal(e.seeds,1);assert.equal(e.seededRevision,3);assert.equal(e.lifeGeneration,0);assert.equal(e.time,2.5);
 e._advanceLife(.1);assert.equal(e.lifeGeneration,1);
 e.setTextMask({...mask([0,0,0,255,255,0],2,4),scrollRevision:1});
 e._uploadPendingMask();e._advanceLife(0);assert.equal(e.seeds,1);assert.equal(e.lifeGeneration,1);
});

check('First scroll metadata and edits at unchanged offsets keep Life additive',()=>{
 const e=engine();
 e.setTextMask({...mask([255,255,0,0,0,0],2,2),scrollRevision:0});
 e._uploadPendingMask();e._advanceLife(0);
 assert.equal(e._lifeNeedsSeed,false);assert.equal(e.lifeGeneration,42);assert.equal(e.emissions,1);
 e.setTextMask({...mask([255,255,0,0,0,0],2,3),scrollRevision:0});
 e._uploadPendingMask();e._advanceLife(0);
 assert.equal(e.lifeGeneration,42);assert.equal(e.emissions,1);
});

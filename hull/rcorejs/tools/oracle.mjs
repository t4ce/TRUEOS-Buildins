#!/usr/bin/env node
// Execute the pinned source itself; no npm installation or network. Remove
// only these two native methods so core-js installs its actual fallback bodies.
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const require=createRequire(import.meta.url);
const core=path.join(root,'core-js-3.51.0/packages/core-js');
const integer=require(path.join(core,'internals/to-integer-or-infinity.js'));
const length=require(path.join(core,'internals/to-length.js'));
for(const name of ['isWellFormed','toWellFormed'])Object.defineProperty(String.prototype,name,{value:undefined,writable:true,configurable:true});
require(path.join(core,'modules/es.string.is-well-formed.js'));
require(path.join(core,'modules/es.string.to-well-formed.js'));
const view=new DataView(new ArrayBuffer(8));
const bits=n=>{view.setFloat64(0,n);return view.getBigUint64(0).toString(16).padStart(16,'0');};
const fromBits=hex=>{view.setBigUint64(0,BigInt('0x'+hex));return view.getFloat64(0);};
const cases=[0,-0,NaN,Infinity,-Infinity,Number.MIN_VALUE,-Number.MIN_VALUE,Number.MAX_VALUE,-Number.MAX_VALUE,0.5,-0.5,1.9,-1.9,Math.PI,-Math.PI,Number.MAX_SAFE_INTEGER-1,Number.MAX_SAFE_INTEGER,Number.MAX_SAFE_INTEGER+1,2**-1022,-(2**-1022)];
let seed=0x12345678;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed;};
for(let i=0;i<1024;i++)cases.push(fromBits(random().toString(16).padStart(8,'0')+random().toString(16).padStart(8,'0')));
const numbers='# input-f64-bits\tToIntegerOrInfinity-bits\tToLength-decimal\n'+cases.map(n=>[bits(n),bits(integer(n)),length(n)].join('\t')).join('\n')+'\n';
const inputs=[[]];const alphabet=[0,0x61,0xd7ff,0xd800,0xdbff,0xdc00,0xdfff,0xe000,0xfffd,0xffff];
function expand(prefix,remaining){if(!remaining)return;for(const unit of alphabet){const next=[...prefix,unit];inputs.push(next);expand(next,remaining-1);}}
expand([],3);
for(let i=0;i<512;i++){const count=random()%17;inputs.push(Array.from({length:count},()=>random()&0xffff));}
const units=s=>Array.from({length:s.length},(_,i)=>s.charCodeAt(i));
const hex=a=>a.length?a.map(v=>v.toString(16).padStart(4,'0')).join(','):'-';
const strings='# input-UTF16\tisWellFormed\ttoWellFormed-UTF16\n'+inputs.map(input=>{const s=String.fromCharCode(...input);return [hex(input),s.isWellFormed(),hex(units(s.toWellFormed()))].join('\t');}).join('\n')+'\n';
const destination=path.join(root,'tests/fixtures');
if(process.argv.includes('--write'))fs.mkdirSync(destination,{recursive:true});
for(const [name,content]of [['numbers.tsv',numbers],['utf16.tsv',strings]]){
    const file=path.join(destination,name);
    if(process.argv.includes('--write'))fs.writeFileSync(file,content);
    else if(fs.readFileSync(file,'utf8')!==content)throw Error(`${name} changed; inspect before using --write`);
}
console.log(`Verified ${cases.length} numeric and ${inputs.length} UTF-16 source cases`);

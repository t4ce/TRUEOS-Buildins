#!/usr/bin/env node
// Development only. Execute the pinned original JS algorithms without npm,
// a browser, network or any production dependency. No upstream source is edited.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const tasks=[];
globalThis.window={document:{},setTimeout(fn){tasks.push(fn);}};
const upstream=relative=>path.join(root,'jquery-main','src',relative);
const load=relative=>import(pathToFileURL(upstream(relative)));
const {jQuery:$}=await load('core.js');
const {toType}=await load('core/toType.js');
const {camelCase}=await load('core/camelCase.js');
const {stripAndCollapse}=await load('core/stripAndCollapse.js');
const {Data}=await load('data/Data.js');
await load('callbacks.js');
await load('deferred.js');
await load('selector/escapeSelector.js');
function isolated(relative,names,values,footer=''){
  const original=fs.readFileSync(upstream(relative),'utf8');
  // Only imports/exports are removed. These two modules' browser-facing methods
  // are defined but never invoked; portable $.param/queue use the real core.
  const body=original.replace(/^import\s+[^;]+;\s*/gm,'').replace(/^export\s*\{[^}]*\};?\s*/gm,'');
  return new Function(...names,body+'\n'+footer)(...values);
}
isolated('serialize.js',['jQuery','toType','rcheckableType'],[$,toType,/^(?:checkbox|radio)$/i]);
isolated('queue.js',['jQuery','dataPriv'],[$,new Data()]);
const dataSource=fs.readFileSync(upstream('data.js'),'utf8');
const start=dataSource.indexOf('function getData( data )');
const end=dataSource.indexOf('\nfunction dataAttr(',start);
if(start<0||end<0)throw Error('Upstream getData extraction changed');
const getData=new Function('rbrace',dataSource.slice(start,end)+'\nreturn getData;')(/^(?:\{[\w\W]*\}|\[[\w\W]*\])$/);
// Array-only init fixture for core collection stack operations. Selector/DOM
// initialization is a separate host contract, not simulated by this oracle.
$.fn.init=function(nodes=[]){$.merge(this,nodes);};$.fn.init.prototype=$.fn;
const cases=[];const add=(op,input,result)=>cases.push({op,input,result});
for(const input of [null,false,0,1,'','abc','😀',[],[1,2],{}, {length:0},{length:'0'},{length:1,0:'x'},{length:3,0:'a',2:'c'},{length:2,0:'a'},{length:1.5,'0.5':'last',0:'a',1:'b'}])add('make_array',input,encode($.makeArray(input)));
for(const input of ['', ' a\t\r\nb ', ' a\u00a0 b ', '\f\v\f', '😀\tfoo'])add('collapse',input,stripAndCollapse(input));
const dataStrings=['true','false','null','undefined','','0','-0','01','1','-1','1.5','1.00','1e3','1e+21','1e-7','0.000001','100000000000000000000','NaN','Infinity','-Infinity',' true','true ','[1,true,null]','{"a":1,"b":[2]}','{"b":1,"2":2,"1":3,"b":4}','{bad}','[1,]','["é","😀"]','{"x":"\\u00e9"}'];
for(const text of dataStrings){let value;try{value=getData(text);}catch{value=text;}add('data',text,encode(value));}
for(const text of ['foo-bar','-ms-transform','-a-b','a--b','a-1','a-É','é-A','--custom-name','FooBar'])add('camel',text,camelCase(text));
for(const text of ['','-','0x','-0x','#a.b','a b','a\tb','a\0b','\x01\x7f','é😀','[x="y"]','--x','a_b'])add('escape',text,$.escapeSelector(text));
for(const n of [0,-0,1,-1,1e-7,1e-6,1e20,1e21,1e23,1000000000000000128,Number.MIN_VALUE,Number.MAX_VALUE,Math.PI,0.30000000000000004])add('number',n,n+'');
let seed=0x12345678;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed;};
function value(depth=0){const n=random();if(depth>=3){return [null,false,true,n%99,`v ${n%17}é`][n%5];}switch(n%6){case 0:return null;case 1:return (n%101)-50;case 2:return `word ${n%11}&é`;case 3:return !!(n&256);case 4:return Array.from({length:n%4},()=>value(depth+1));default:return Object.fromEntries(Array.from({length:n%4},(_,i)=>['k'+i,value(depth+1)]));}}
const params=[{},null,{a:1,b:null,c:false},{a:[1,2,3]},{a:[{b:1},{b:2}]},{'a[]':[1,{x:2}]},{'10':'ten','2':'two',b:'three'},[{name:'a b',value:'é\n😀'},{name:'a',value:null}],{a:[[],{},[2,3]]},{'!()\'*~':'!/ ?#'}];
for(let i=0;i<180;i++)params.push({a:value(),b:value()});
for(const input of params)for(const traditional of [false,true])add('param',{value:input,traditional},$.param(input,traditional));
for(let i=0;i<100;i++){const target={a:[1,2],b:{x:1},z:true};const source={a:value(),b:value()};for(const deep of [false,true])add('extend',{target,source,deep},encode($.extend(deep,{},target,source)));}
for(const deep of [false,true]){const source=JSON.parse('{"__proto__":{"polluted":true},"ok":2}');add('extend',{target:{},source,deep},encode($.extend(deep,{},source)));}
for(const nodes of [[],[1],[1,2,3,4,5],[4,4,2]])for(let index=-7;index<7;index++)add('collection',{nodes,index},{get:$(nodes).get(index)??null,eq:$(nodes).eq(index).toArray(),first:$(nodes).first().toArray(),last:$(nodes).last().toArray(),even:$(nodes).even().toArray(),odd:$(nodes).odd().toArray(),slice:$(nodes).slice(index,-1).toArray(),end:$(nodes).eq(index).end().toArray()});
for(let flags=0;flags<16;flags++){
  const options=['once','memory','unique','stopOnFalse'].filter((_,i)=>flags&(1<<i)).join(' ');const list=$.Callbacks(options);const trace=[];
  function a(value){trace.push(`a:${this}:${value}`);}function b(value){trace.push(`b:${this}:${value}`);return false;}function c(value){trace.push(`c:${this}:${value}`);}function d(value){trace.push(`d:${this}:${value}`);}
  list.add(a,b,a);list.fireWith('one',[1]);list.remove(a);list.add(c);list.fireWith('two',[2]);list.lock();list.add(d);list.fireWith('three',[3]);
  add('callbacks',options,{trace,disabled:list.disabled(),locked:list.locked(),fired:list.fired(),has:list.has()});
}
// Explicitly controlled upstream timeout queue, matching native TaskQueue.
{
  const d=$.Deferred(),trace=[];
  d.progress(x=>trace.push('progress:'+x));d.done(x=>trace.push('done:'+x));
  d.then(x=>{trace.push('then:'+x);return x+1;}).done(x=>trace.push('child:'+x));
  d.notify(2);d.resolve(7);d.reject(8);d.notify(3);d.done(x=>trace.push('late:'+x));trace.push('before-drain');while(tasks.length)tasks.shift()();
  add('deferred',null,{trace,state:d.state()});
}
for(const kind of ['fx','custom']){
  const owner={},trace=[];let next;const job=id=>function(n){trace.push('job:'+id);next=n;};
  $.queue(owner,kind,[job(1),job(2)]);$._queueHooks(owner,kind).empty.add(()=>trace.push('empty'));
  const lengths=[$.queue(owner,kind).length];$.dequeue(owner,kind);lengths.push($.queue(owner,kind).length);next();lengths.push($.queue(owner,kind).length);next();lengths.push($.queue(owner,kind).length);add('queue',kind,{trace,lengths});
}
function encode(value){if(typeof value==='number'&&!Number.isFinite(value))return {$number:value+''};if(value===undefined)return {$undefined:true};if(Array.isArray(value))return value.map(encode);if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).map(([k,v])=>[k,encode(v)]));return value;}
const output=JSON.stringify({description:'Expected results from vendored upstream jQuery source; see tools/oracle.mjs',cases},null,2)+'\n';
const destination=path.join(root,'tests/fixtures/portable.json');
if(process.argv.includes('--write')){fs.mkdirSync(path.dirname(destination),{recursive:true});fs.writeFileSync(destination,output);console.log(`Recorded ${cases.length} upstream cases`);}
else{if(fs.readFileSync(destination,'utf8')!==output)throw Error('Oracle differs: inspect changes, then use --write');console.log(`Verified ${cases.length} upstream cases`);}

#!/usr/bin/env node
// Development oracle: force the original pinned core-js URL fallback, never
// Node's URL implementation. No upstream edits, npm install or live requests.
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const require=createRequire(import.meta.url);
globalThis.URL=undefined;globalThis.URLSearchParams=undefined;
require(path.resolve(root,'../../rcorejs/core-js-3.51.0/packages/core-js/modules/web.url.constructor.js'));
const fields=['href','origin','protocol','username','password','host','hostname','port','pathname','search','hash'];
const cases=[];
function url(args,assign=[]){let expected;try{const u=new URL(...args);for(const[k,v]of assign)u[k]=v;expected=Object.fromEntries(fields.map(k=>[k,u[k]]));}catch(e){expected={error:e.name};}cases.push({kind:'url',args,assign,expected});}
const roots=['https://user:pass@EXAMPLE.org:443/a/b?x=one%20two#top','http://127.1:8080/','https://[::1]:443/a','https://münich.example/é','file:///C:/a/b','ftp://example.org/a','ws://example.org:80/a','mailto:person@example.org','data:text/plain,hello','custom://example.org/a'];
for(const input of roots){url([input]);for(const pair of [['protocol','http:'],['username','a b'],['password','p@ss'],['host','other.test:8090'],['hostname','new.test'],['port','8443'],['port','bad'],['pathname','/new path'],['search','q=one two&x=~'],['hash','new value'],['href','https://new.test/?a=2']])url([input],[pair]);}
for(const input of ['../x','?q=1','#hash','/a b','//other.test/x','https:other.test/a'])url([input,'https://example.org/dir/page']);
for(const input of ['relative','http://[bad','https://x:99999','https://',''])url([input]);
url(['https://example.org','invalid base']);
for(const args of [[],[''],[null],['?a=1&a=2&x=one+two'],['bad=%E0%A4%A&zero=%00&x=%zz'],[{q:'é a',n:null}],[ [['b','1'],['a','2'],['a','3']] ]]){
    for(const operations of [[],[['append','x','+ ~'],['set','a','new']],[['delete','a','1'],['sort']]]){
        const p=new URLSearchParams(...args);for(const[method,...values]of operations)p[method](...values);
        cases.push({kind:'query',args,operations,expected:{serialized:p.toString(),entries:[...p],size:p.size}});
    }
}
// Preserve input record insertion order through Rust's JSON object map.
for(const c of cases)c.argsJSON=JSON.stringify(c.args);
const file=path.join(root,'src/fixtures/url-corejs.json');const output=JSON.stringify({source:'core-js 3.51.0 forced fallback',cases},null,2)+'\n';
if(process.argv.includes('--write')){fs.mkdirSync(path.dirname(file),{recursive:true});fs.writeFileSync(file,output);}
else if(fs.readFileSync(file,'utf8')!==output)throw Error('URL oracle changed; inspect before --write');
console.log(`Verified ${cases.length} original core-js URL/URLSearchParams cases`);

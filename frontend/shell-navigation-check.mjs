import fs from 'node:fs';
import ts from 'typescript';
import assert from 'node:assert/strict';
const source=fs.readFileSync('src/composables/useNavigationTrail.ts','utf8').replace('import { getModuleFrontends } from "../modules/registry";', 'const getModuleFrontends = () => [{id:"a",routes:[{path:"/a",name:"a"},{path:"/detail",name:"detail"}]},{id:"b",routes:[{path:"/b",name:"b"}]}];');
const output=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText;
const temporary=new URL('./.shell-navigation-check-runtime.mjs',import.meta.url);
fs.writeFileSync(temporary,output);
try {
 const {initNavigationTrail,useNavigationTrail}=await import(temporary.href);
 let afterEach; let current={fullPath:'',path:'',meta:{},matched:[]};
 const history={state:{}};
 const resolve=(fullPath)=>({matched:fullPath.startsWith('/missing')?[]:[{}]});
 initNavigationTrail({options:{history},afterEach:fn=>afterEach=fn,resolve});
 const trail=useNavigationTrail();
 const paths=()=>trail.visits.value.map(v=>v.fullPath);
 const go=(fullPath,name,position,back=null,forward=null)=>{const to={fullPath,path:fullPath.split('?')[0],name,matched:[{name,path:fullPath.split('?')[0]}],meta:{titleKey:name}};history.state={position,back,forward};afterEach(to,current);current=to;};
 go('/a','a',0);assert.deepEqual(paths(),['/a']);assert.equal(trail.canBack.value,false);
 go('/detail?q=one#x','detail',1,'/a');assert.deepEqual(paths(),['/a','/detail?q=one#x']);
 go('/detail?q=two','detail',2,'/detail?q=one#x');assert.equal(paths().length,3);
 go('/detail?q=one#x','detail',1,'/a','/detail?q=two');assert.equal(paths().length,2);assert.equal(trail.canForward.value,true);
 go('/detail?q=two','detail',2,'/detail?q=one#x');assert.equal(paths().length,3);
 go('/detail?q=replaced','detail',2,'/detail?q=one#x');assert.deepEqual(paths(),['/a','/detail?q=one#x','/detail?q=replaced']);
 go('/a','a',0,null,'/detail?q=one#x');go('/detail?q=branch','detail',1,'/a');assert.deepEqual(paths(),['/a','/detail?q=branch']);
 go('/b','b',2,'/detail?q=branch');assert.deepEqual(paths(),['/b']);
 go('/detail?q=branch','detail',1,'/a','/b');assert.deepEqual(paths(),['/detail?q=branch']);
 go('/detail?q=last','detail',2,'/detail?q=branch');assert.deepEqual(paths(),['/detail?q=branch','/detail?q=last']);
 console.log('PASS: direct link, same-module route ownership, query/hash, back, forward, replace, branch pruning, cross-module reset');
} finally {fs.unlinkSync(temporary);}

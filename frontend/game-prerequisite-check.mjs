import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import assert from 'node:assert/strict';
const source=readFileSync('src/composables/useGamePrerequisites.ts','utf8').replace(/^import .*;$/gm,'');
const javascript=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
function harness(){let probes=0,installs=0;const states=[];const context={exports:{},navigator:{userAgent:'Windows'},ref:value=>({value}),call:async(command)=>{if(command==='game_dependencies_probe'){probes++;return states.shift()??{state:'missing',missingVcDlls:['vcruntime140.dll'],missingPackages:[]};}installs++;return states.shift()??{state:'installed'};}};vm.runInNewContext(javascript,context);return {api:context.exports,states,counts:()=>({probes,installs})};}
{
 const {api,counts}=harness();const awaiting=api.ensureGamePrerequisites('instance-a');await new Promise(resolve=>setImmediate(resolve));api.cancelGamePrerequisites();assert.equal(await awaiting,false);assert.equal(counts().installs,0);
}
{
 const {api,states,counts}=harness();const pending=api.ensureGamePrerequisites('instance-a');await new Promise(resolve=>setImmediate(resolve));assert.equal(await api.ensureGamePrerequisites('instance-b'),false);states.push({state:'installed'},{state:'ready'});await api.installGamePrerequisites();assert.equal(await pending,true);assert.equal(counts().installs,1);
}
{
 const {api,states}=harness();const pending=api.ensureGamePrerequisites('instance-a');await new Promise(resolve=>setImmediate(resolve));states.push({state:'restart-required'});await api.installGamePrerequisites();assert.equal(api.gameDependencyPrompt.value,true);assert.equal(api.gameDependencyError.value,'restart-required');api.cancelGamePrerequisites();assert.equal(await pending,false);
}
{
 const {api,states}=harness();const pending=api.ensureGamePrerequisites('instance-a');await new Promise(resolve=>setImmediate(resolve));states.push({state:'installed'},{state:'missing'});await api.installGamePrerequisites();assert.equal(api.gameDependencyError.value,'dependency-still-missing');api.cancelGamePrerequisites();assert.equal(await pending,false);
}
console.log('4 game prerequisite continuation checks passed');

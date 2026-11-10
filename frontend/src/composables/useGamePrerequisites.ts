import { ref } from 'vue';
import { call } from '../api/core';
export interface GameDependencyStatus { state:string; missingVcDlls:string[];missingPackages:{name:string;minVersion:string;architecture:string}[] }
export const gameDependencyPrompt=ref(false);
export const gameDependencyInstalling=ref(false);
export const gameDependencyError=ref('');
export const gameDependencyStatus=ref<GameDependencyStatus|null>(null);
let currentName='';let pending:Promise<boolean>|null=null;let finish:((value:boolean)=>void)|undefined;
export async function ensureGamePrerequisites(name:string):Promise<boolean>{
  // Android targets have their own package manager; never probe Windows dependencies there.
  if(navigator.userAgent.includes('Android'))return true;
  if(pending)return false;
  const status=await call<GameDependencyStatus>('game_dependencies_probe',{name});
  if(status.state==='ready')return true;
  currentName=name;gameDependencyStatus.value=status;gameDependencyError.value='';gameDependencyPrompt.value=true;
  pending=new Promise<boolean>(resolve=>{finish=resolve;});return pending;
}
export function cancelGamePrerequisites(){if(gameDependencyInstalling.value)return;gameDependencyPrompt.value=false;finish?.(false);finish=undefined;pending=null;}
export async function installGamePrerequisites(){
  if(gameDependencyInstalling.value)return;gameDependencyInstalling.value=true;gameDependencyError.value='';
  try{
    const outcome=await call<{state:string}>('game_dependencies_install',{name:currentName});
    if(!['installed','already-installed','ready'].includes(outcome.state)){gameDependencyError.value=outcome.state;return;}
    const status=await call<GameDependencyStatus>('game_dependencies_probe',{name:currentName});gameDependencyStatus.value=status;
    if(status.state!=='ready'){gameDependencyError.value='dependency-still-missing';return;}
    gameDependencyPrompt.value=false;finish?.(true);finish=undefined;pending=null;
  }catch(error){gameDependencyError.value=String(error);}finally{gameDependencyInstalling.value=false;}
}

import { ref } from 'vue';
import { componentsProbe, componentsInstall } from '../api/components';
export const componentPrompt=ref(false);
export const componentInstalling=ref(false);
export const componentError=ref('');
let pending:Promise<boolean>|null=null;
let finish:((ok:boolean)=>void)|undefined;
export async function ensureLipComponents():Promise<boolean>{
  try {await componentsProbe('dotnet10');await componentsProbe('lip');return true;} catch { /* explicit user consent below */ }
  if(pending)return pending;
  componentPrompt.value=true;componentError.value='';
  pending=new Promise<boolean>(resolve=>{finish=resolve;});return pending;
}
export function cancelComponents(){if(componentInstalling.value)return;componentPrompt.value=false;finish?.(false);finish=undefined;pending=null;}
export async function installComponents(){if(componentInstalling.value)return;componentInstalling.value=true;componentError.value='';try{await componentsInstall('lip');await componentsProbe('dotnet10');await componentsProbe('lip');componentPrompt.value=false;finish?.(true);finish=undefined;pending=null;}catch(e){componentError.value=String(e);}finally{componentInstalling.value=false;}}

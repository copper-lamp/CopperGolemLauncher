import { shallowReactive } from 'vue';
import { settingsCategories, type SettingsCategory } from './settings-categories';
export interface SettingField { id:string; titleKey:string; type:'boolean'|'number'|'string'|'enum'; default:boolean|number|string; min?:number; max?:number; options?:{value:string;titleKey:string}[] }
export interface SettingsContribution {id:string;category:SettingsCategory;titleKey:string;order?:number;fields:SettingField[]}
export interface SettingsOwner {id:string;namespace:string;read:(key:string)=>Promise<unknown>;write:(key:string,value:unknown)=>Promise<void>;t:(key:string)=>string}
export const settingsContributions=shallowReactive(new Map<string,{owner:SettingsOwner;section:SettingsContribution}>());
const lexical=/^[a-zA-Z0-9_-]{1,64}$/;
export function registerSettings(owner:SettingsOwner,section:SettingsContribution):()=>void {
 if(!lexical.test(section.id)||!settingsCategories.includes(section.category)||!section.titleKey.startsWith(owner.namespace+'.')||!Array.isArray(section.fields)||section.fields.length>64)throw new Error('Invalid settings contribution');
 const ids=new Set<string>();
 for(const field of section.fields){if(!lexical.test(field.id)||ids.has(field.id)||!field.titleKey.startsWith(owner.namespace+'.')||!['boolean','number','string','enum'].includes(field.type))throw new Error('Invalid settings field');ids.add(field.id);if(field.type==='enum'&&(!field.options?.length||field.options.length>64||field.options.some(o=>!o.titleKey.startsWith(owner.namespace+'.'))))throw new Error('Invalid settings options');validateSetting(field,field.default);}
 const key=owner.id+':'+section.id;if(settingsContributions.has(key))throw new Error('Duplicate settings contribution');
 const record={owner,section:structuredClone(section)};settingsContributions.set(key,record);return()=>{if(settingsContributions.get(key)===record)settingsContributions.delete(key);};
}
export function validateSetting(field:SettingField,value:unknown):void {if(field.type==='boolean'&&typeof value!=='boolean'||field.type==='number'&&(typeof value!=='number'||!Number.isFinite(value)||(field.min!==undefined&&value<field.min)||(field.max!==undefined&&value>field.max))||['string','enum'].includes(field.type)&&(typeof value!=='string'||value.length>4096)||field.type==='enum'&&!field.options?.some(o=>o.value===value))throw new Error('Invalid setting value');}
export function disposeModuleSettings(id:string):void {for(const [key,entry] of settingsContributions)if(entry.owner.id===id)settingsContributions.delete(key);}

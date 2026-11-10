import { call } from './core';
export type ComponentId = 'lip' | 'dotnet10';
export interface ComponentStatus { id: ComponentId; state: string; version: string|null; availableVersion: string|null; stage: string|null; error: string|null; supported: boolean }
export const componentsCancel = () => call<void>('components_cancel');
export const componentsList = () => call<ComponentStatus[]>('components_list');
export const componentsProbe = (id: ComponentId) => call<boolean>('components_probe',{id});
export const componentsCheck = (id: ComponentId) => call<ComponentStatus>('components_check',{id});
export const componentsInstall = (id: ComponentId) => call<void>('components_install',{id});

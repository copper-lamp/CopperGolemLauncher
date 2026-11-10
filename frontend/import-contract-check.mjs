import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('./src/services/import-version.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { compareImportVersions: cmp } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
for (const [a, b, expected] of [
  ['1.2.3','1.2.2',1], ['1.2.2','1.2.3',-1], ['1.2.3+build','1.2.3',0],
  ['1.0.0-alpha','1.0.0',-1], ['1.0.0-beta.2','1.0.0-beta.11',-1],
  ['1.0.0-2','1.0.0-alpha',-1], ['1.0.0-alpha','1.0.0-alpha.1',-1],
  ['99999999999999999999.0.0','10000000000000000000.0.0',1],
  ['v1.2.3','1.2.3',null], ['1.0.0-01','1.0.0',null], ['01.2.3','1.2.3',null],
]) assert.equal(cmp(a,b), expected, `${a} versus ${b}`);
console.log('Import SemVer upgrade/downgrade and unknown-version cases passed.');

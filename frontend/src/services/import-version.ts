/** Strict SemVer precedence; unknown/non-SemVer values are never guessed. */
export function compareImportVersions(incoming: string, installed: string): number | null {
  const pattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
  const a = pattern.exec(incoming); const b = pattern.exec(installed);
  if (!a || !b) return null;
  const numeric = (x: string, y: string) => x.length === y.length ? (x === y ? 0 : x > y ? 1 : -1) : x.length > y.length ? 1 : -1;
  const validPre = (value: string | undefined) => !value?.split('.').some(part => /^\d+$/.test(part) && part.length > 1 && part.startsWith('0'));
  if (!validPre(a[4]) || !validPre(b[4])) return null;
  for (let i = 1; i <= 3; i++) { const result = numeric(a[i]!, b[i]!); if (result) return result; }
  if (a[4] === b[4]) return 0;
  if (!a[4]) return 1;
  if (!b[4]) return -1;
  const left = a[4].split('.'); const right = b[4].split('.');
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const x = left[i]; const y = right[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    if (x === y) continue;
    const xn = /^\d+$/.test(x); const yn = /^\d+$/.test(y);
    if (xn && yn) return numeric(x, y);
    if (xn !== yn) return xn ? -1 : 1;
    return x > y ? 1 : -1;
  }
  return 0;
}

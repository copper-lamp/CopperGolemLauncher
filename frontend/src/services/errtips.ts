// Structured diagnostics contain no invoke arguments, raw logs, URLs or user paths.
import { shallowRef } from 'vue';
export interface ErrorContext {
  schema_version?: number; error_id?: string; platform?: string; code?: string; source?: string;
  timeout_scope?: 'ipc' | 'daemon_rpc' | 'network' | 'io'; http_status?: number;
  os_code?: number; host?: string; retry_safe?: boolean; module?: string;
  operation?: string; stage?: string; job_id?: string;
}
export interface DomainFailure extends ErrorContext { message?: string }
export interface ErrorReport { id: string; context: ErrorContext; rule: string; created: number }
export const errorReports = shallowRef<ErrorReport[]>([]);
export const activeErrorReport = shallowRef<ErrorReport | null>(null);
let sequence = 0;
const reportsByError = new WeakMap<object, ErrorReport>();
// Match existing page Toast messages to the exact error; never export these ephemeral strings.
const toastReports = new Map<string, ErrorReport>();
export function reportForMessage(message: string): ErrorReport | undefined { return toastReports.get(message.replace(/^KernelApiError: /, '')); }
const token = (v: unknown) => typeof v === 'string' && /^[a-zA-Z0-9_.:-]{1,120}$/.test(v) ? v : undefined;
const publicHosts = new Set(['github.com', 'api.github.com', 'raw.githubusercontent.com', 'objects.githubusercontent.com', 'licensing.mp.microsoft.com', 'displaycatalog.mp.microsoft.com']);
export function safeContext(raw: ErrorContext): ErrorContext {
  const scope = ['ipc', 'daemon_rpc', 'network', 'io'].includes(raw.timeout_scope ?? '') ? raw.timeout_scope : undefined;
  return { schema_version: 1, error_id: token(raw.error_id), platform: token(raw.platform),
    code: token(raw.code) ?? 'unknown', source: token(raw.source) ?? 'unknown', timeout_scope: scope,
    module: token(raw.module), operation: token(raw.operation), stage: token(raw.stage), job_id: token(raw.job_id),
    http_status: Number.isInteger(raw.http_status) && raw.http_status! >= 100 && raw.http_status! <= 599 ? raw.http_status : undefined,
    os_code: Number.isInteger(raw.os_code) ? raw.os_code : undefined,
    host: raw.host && publicHosts.has(raw.host) ? raw.host : undefined,
    // Caller evidence alone never grants a retry action.
    retry_safe: false };
}
export function ruleFor(c: ErrorContext): string {
  if (c.timeout_scope === 'ipc') return 'ipc_timeout';
  if (c.timeout_scope === 'daemon_rpc') return 'daemon_timeout';
  if (c.timeout_scope === 'network' && c.source === 'reqwest') return 'network_timeout';
  if (c.code === 'ipc_unavailable') return 'ipc_unavailable';
  if (c.http_status === 429) return 'rate_limit';
  if (c.http_status === 401 || c.http_status === 403) return 'access_rejected';
  if (c.http_status && c.http_status >= 500) return 'service_unavailable';
  const exact: Record<string, string> = {
    permission_denied: 'permission_denied', disk_full: 'disk_full', not_found: 'not_found', conflict: 'conflict', already_exists: 'conflict',
    invalid_argument: 'invalid_argument', invalid_response: 'invalid_response', network_connect: 'network_connect', network_request: 'network_request',
    interaction_required: 'interaction_required', account_changed: 'account_changed', invalid_identity: 'account_changed',
    no_content_key: 'entitlement_unknown', service_refused: 'service_refused', malformed_license_response: 'entitlement_unknown',
    account_unclassified: 'entitlement_unknown', credential_storage: 'credential_storage',
    cancelled: 'cancelled', canceled: 'cancelled',
  };
  return exact[c.code ?? ''] ?? 'unknown';
}
export function reportError(error: unknown, extra: ErrorContext = {}): ErrorReport {
  if (typeof error === 'object' && error !== null) {
    const old = reportsByError.get(error); if (old) return old;
  }
  const raw = typeof error === 'object' && error !== null && 'context' in error ? (error as {context: ErrorContext}).context : {};
  const context = safeContext({ ...raw, ...extra });
  const id = context.error_id ?? `ui-${Date.now()}-${++sequence}`;
  const existing = errorReports.value.find(r => r.id === id);
  const report = existing ?? { id, context, rule: ruleFor(context), created: Date.now() };
  if (!existing) errorReports.value = [...errorReports.value.slice(-49), report];
  if (typeof error === 'object' && error !== null) {
    reportsByError.set(error, report);
    if (error instanceof Error) {
      if (toastReports.size >= 50) toastReports.delete(toastReports.keys().next().value!);
      toastReports.set(error.message, report);
    }
  }
  return report;
}
export function showErrorDetails(error: unknown, extra: ErrorContext = {}): void { activeErrorReport.value = reportError(error, extra); }
export function reportDomainFailure(input: DomainFailure): ErrorReport {
  const report = reportError({}, input);
  // Explicit business failures are user operations; polling failures collected by call() do not open a modal.
  activeErrorReport.value = report;
  return report;
}
// Only the host can register an operation; report-provided retry_safe never grants permission.
export interface SafeRetryContract {
  isIdempotent: true;
  isAuthorized: () => boolean;
  isPreviousOperationStopped: () => Promise<boolean>;
  execute: () => Promise<void>;
}
const retryContracts = new Map<string, SafeRetryContract>();
const retriesInFlight = new Set<string>();
export function registerSafeRetry(operation: string, contract: SafeRetryContract): () => void {
  if (!token(operation) || contract.isIdempotent !== true) throw new Error('Invalid retry contract');
  retryContracts.set(operation, contract);
  return () => { if (retryContracts.get(operation) === contract) retryContracts.delete(operation); };
}
export async function runSafeRetry(report: ErrorReport): Promise<boolean> {
  const operation = report.context.operation;
  const contract = operation ? retryContracts.get(operation) : undefined;
  if (!operation || !contract || retriesInFlight.has(operation)) return false;
  retriesInFlight.add(operation);
  try {
    if (!contract.isAuthorized() || !await contract.isPreviousOperationStopped()) return false;
    // Re-check revocation/registration after awaiting kernel state.
    if (!contract.isAuthorized() || retryContracts.get(operation) !== contract) return false;
    await contract.execute();
    return true;
  } finally { retriesInFlight.delete(operation); }
}
export function diagnosticText(report: ErrorReport): string {
  return JSON.stringify({ version: 1, error_id: report.id, rule: report.rule, context: safeContext(report.context) }, null, 2);
}

// Addons can contribute only exact codes inside their own namespace. No executable actions or regex.
interface Contribution { code: string; titleKey: string; bodyKey: string }
const contributions = new Map<string, readonly Contribution[]>();
export function registerErrorRules(moduleId: string, rules: readonly Contribution[], isAuthorized: () => boolean): () => void {
  if (!/^[a-z0-9][a-z0-9-]{0,63}$/.test(moduleId) || !isAuthorized() || rules.length > 64) throw new Error('Invalid error-rule registration');
  const copy = rules.map(r => {
    if (!r.code.startsWith(`${moduleId}.`) || !token(r.code) || !r.titleKey.startsWith(`module.${moduleId}.`) || !r.bodyKey.startsWith(`module.${moduleId}.`)) throw new Error('Error-rule namespace violation');
    return Object.freeze({ ...r });
  });
  contributions.set(moduleId, copy);
  contributionAuthorization.set(moduleId, isAuthorized);
  return () => { if (contributions.get(moduleId) === copy) { contributions.delete(moduleId); contributionAuthorization.delete(moduleId); } };
}
const contributionAuthorization = new Map<string, () => boolean>();
export function contributedText(report: ErrorReport): Contribution | undefined {
  if (report.rule !== 'unknown') return;
  const module = report.context.module;
  if (!module) return;
  try { if (!contributionAuthorization.get(module)?.()) return; } catch { return; }
  return contributions.get(module)?.find(r => r.code === report.context.code);
}

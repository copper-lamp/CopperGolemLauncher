// 更新面板的展示格式化。
//
// 单独成文件而不是塞进组件：这些是纯函数，与组件的渲染逻辑无关，
// 且「剩余时间怎么写」是需要和 i18n 词条一起审阅的文案决策。

/**
 * 剩余时间。
 *
 * 不做逐秒精度：下载速度本就在波动，「剩 1 分 47 秒」这种精度是假精确。
 * 只到分钟，且小于一分钟时说「不到一分钟」而不是「剩 23 秒」。
 */
export function formatEta(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  if (total < 60) return "<1m";
  if (total < 3600) return `${Math.round(total / 60)}m`;
  const hours = Math.floor(total / 3600);
  const minutes = Math.round((total % 3600) / 60);
  return minutes === 0 ? `${hours}h` : `${hours}h${minutes}m`;
}

/** 秒级时间戳 → 本地时间 `HH:MM`。 */
export function formatTime(epochSeconds: number): string {
  const date = new Date(epochSeconds * 1000);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}
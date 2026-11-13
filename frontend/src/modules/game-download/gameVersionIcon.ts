import releaseIcon from "./assets/release.jpg";
import previewIcon from "./assets/pre.jpg";
import type { GameVersionKind } from "./api";

/** User-supplied Minecraft artwork, shared by installation cards and details. */
export function gameVersionIcon(kind: GameVersionKind): string {
  return kind === "preview" ? previewIcon : releaseIcon;
}

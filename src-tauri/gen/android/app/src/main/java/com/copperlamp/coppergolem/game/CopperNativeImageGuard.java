package com.copperlamp.coppergolem.game;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.concurrent.ConcurrentHashMap;

/**
 * 原生库镜像处理。
 *
 * 从用户导入的 APK 中解出的 {@code .so} 在写入缓存后需要经过
 * {@link CopperNativeBridge} 的重写，否则 Minecraft 引擎在
 * {@code System.load} 之后的初始化阶段会失败。这里用「token + 长度 + mtime」
 * 三元组做幂等标记，重复启动时直接跳过重写，避免每次启动都重写几十 MB。
 */
public final class CopperNativeImageGuard {
    public static final String TOKEN = "copper_img_v1";

    private static final ConcurrentHashMap<String, FileState> CLEAN_FILES = new ConcurrentHashMap<>();

    private CopperNativeImageGuard() {
    }

    /** 该文件是否已被处理过（内存缓存或磁盘标记命中）。 */
    public static boolean shouldProcess(File soFile) {
        if (!isEligible(soFile)) {
            return false;
        }
        String path = soFile.getAbsolutePath();
        FileState state = FileState.from(soFile);
        FileState cached = CLEAN_FILES.get(path);
        if (state.equals(cached) || state.equals(readCleanMarker(soFile))) {
            CLEAN_FILES.put(path, state);
            return false;
        }
        return scan(soFile);
    }

    /** 命中磁盘标记则跳过，否则按需处理。 */
    public static boolean processIfNeeded(File soFile) {
        return process(soFile, false);
    }

    /** 强制重新处理：清掉内存与磁盘标记后重写。 */
    public static boolean processRequired(File soFile) {
        return process(soFile, true);
    }

    public static int processDirectory(File dir) {
        if (dir == null || !dir.exists()) {
            return 0;
        }
        File[] files = dir.listFiles();
        if (files == null) {
            return 0;
        }
        int count = 0;
        for (File file : files) {
            if (file.isDirectory()) {
                count += processDirectory(file);
            } else if (file.getName().endsWith(".so") && processIfNeeded(file)) {
                count++;
            }
        }
        return count;
    }

    private static boolean process(File soFile, boolean ignoreMarker) {
        if (!isEligible(soFile)) {
            return false;
        }
        if (!CopperNativeBridge.ensureGxCoreLoaded()) {
            return false;
        }
        if (ignoreMarker) {
            CLEAN_FILES.remove(soFile.getAbsolutePath());
            clearCleanMarker(soFile);
        } else if (!shouldProcess(soFile)) {
            return true;
        }

        if (!scan(soFile)) {
            markClean(soFile);
            return true;
        }
        try {
            ensureWritable(soFile);
            File tempFile = new File(soFile.getAbsolutePath() + ".img.tmp");
            if (tempFile.exists() && !tempFile.delete()) {
                throw new IOException("无法删除残留临时文件: " + tempFile.getAbsolutePath());
            }
            if (!CopperNativeBridge.rewriteImage(soFile.getAbsolutePath(), tempFile.getAbsolutePath())) {
                tempFile.delete();
                return false;
            }
            Files.move(tempFile.toPath(), soFile.toPath(), StandardCopyOption.REPLACE_EXISTING);
            soFile.setReadable(true, true);
            soFile.setReadOnly();
            markClean(soFile);
            return true;
        } catch (Exception error) {
            CLEAN_FILES.remove(soFile.getAbsolutePath());
            clearCleanMarker(soFile);
            return false;
        }
    }

    private static boolean isEligible(File soFile) {
        return soFile != null && soFile.isFile() && soFile.length() > 0L && soFile.getName().endsWith(".so");
    }

    private static void ensureWritable(File file) throws IOException {
        File parent = file.getParentFile();
        if (parent != null && !parent.exists() && !parent.mkdirs()) {
            throw new IOException("无法创建目录: " + parent.getAbsolutePath());
        }
        if (file.exists() && !file.setWritable(true, true) && !file.canWrite()) {
            throw new IOException("无法置为可写: " + file.getAbsolutePath());
        }
    }

    private static boolean scan(File soFile) {
        return CopperNativeBridge.ensureGxCoreLoaded()
                && CopperNativeBridge.scanImage(soFile.getAbsolutePath());
    }

    private static File cleanMarker(File file) {
        File parent = file.getParentFile();
        String name = "." + file.getName() + "." + TOKEN + ".ok";
        return parent == null ? new File(name) : new File(parent, name);
    }

    private static FileState readCleanMarker(File file) {
        File marker = cleanMarker(file);
        if (!marker.isFile()) {
            return null;
        }
        try {
            String[] parts = new String(Files.readAllBytes(marker.toPath()), StandardCharsets.UTF_8)
                    .trim().split(":");
            if (parts.length != 3 || !TOKEN.equals(parts[0])) {
                return null;
            }
            return new FileState(Long.parseLong(parts[1]), Long.parseLong(parts[2]));
        } catch (Exception ignored) {
            return null;
        }
    }

    private static void markClean(File file) {
        FileState state = FileState.from(file);
        CLEAN_FILES.put(file.getAbsolutePath(), state);
        try {
            File marker = cleanMarker(file);
            File parent = marker.getParentFile();
            if (parent != null && !parent.exists() && !parent.mkdirs()) {
                return;
            }
            String data = TOKEN + ":" + state.length + ":" + state.lastModified;
            Files.write(marker.toPath(), data.getBytes(StandardCharsets.UTF_8));
        } catch (Exception ignored) {
            // 标记写失败只影响下次是否重写，不影响本次加载结果。
        }
    }

    private static void clearCleanMarker(File file) {
        try {
            File marker = cleanMarker(file);
            if (marker.exists()) {
                marker.delete();
            }
        } catch (Exception ignored) {
        }
    }

    private static final class FileState {
        private final long length;
        private final long lastModified;

        private FileState(long length, long lastModified) {
            this.length = length;
            this.lastModified = lastModified;
        }

        static FileState from(File file) {
            return new FileState(file.length(), file.lastModified());
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof FileState)) {
                return false;
            }
            FileState that = (FileState) other;
            return length == that.length && lastModified == that.lastModified;
        }

        @Override
        public int hashCode() {
            long value = length ^ (length >>> 32) ^ lastModified ^ (lastModified >>> 32);
            return (int) value;
        }
    }
}

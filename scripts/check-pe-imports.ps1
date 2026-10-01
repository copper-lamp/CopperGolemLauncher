# List a PE file's imported DLLs and functions, and check that every named import
# resolves. Purpose: pinpoint which import is missing when a binary dies with
# 0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND.
param(
  [string[]]$Path,
  [switch]$Quiet
)

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class PeImports {
  public class Import { public string Dll; public List<string> Funcs = new List<string>(); }

  public static List<Import> Read(string path) {
    byte[] b = File.ReadAllBytes(path);
    int pe = BitConverter.ToInt32(b, 0x3C);
    int opt = pe + 24;
    ushort magic = BitConverter.ToUInt16(b, opt);
    bool pe32plus = magic == 0x20b;
    int numDirs = BitConverter.ToInt32(b, opt + (pe32plus ? 108 : 92));
    int dirOff = opt + (pe32plus ? 112 : 96);
    int impRva = 0;
    if (numDirs > 1) impRva = BitConverter.ToInt32(b, dirOff + 8);
    if (impRva == 0) return new List<Import>();
    int sections = BitConverter.ToInt16(b, pe + 6);
    int optSize = BitConverter.ToInt16(b, pe + 20);
    int secOff = opt + optSize;
    Func<int,int> toOff = (rva) => {
      for (int i = 0; i < sections; i++) {
        int s = secOff + i * 40;
        int va = BitConverter.ToInt32(b, s + 12);
        int vs = BitConverter.ToInt32(b, s + 8);
        int raw = BitConverter.ToInt32(b, s + 20);
        int rawSize = BitConverter.ToInt32(b, s + 16);
        if (rva >= va && rva < va + Math.Max(vs, rawSize)) return raw + (rva - va);
      }
      return -1;
    };
    var list = new List<Import>();
    int d = toOff(impRva);
    if (d < 0) return list;
    while (true) {
      int oft = BitConverter.ToInt32(b, d);
      int nameRva = BitConverter.ToInt32(b, d + 12);
      int iat = BitConverter.ToInt32(b, d + 16);
      if (oft == 0 && nameRva == 0 && iat == 0) break;
      int nOff = toOff(nameRva);
      if (nOff < 0) break;
      var im = new Import { Dll = ReadCString(b, nOff) };
      int thunkRva = oft != 0 ? oft : iat;
      int t = toOff(thunkRva);
      while (t > 0) {
        ulong v = pe32plus ? BitConverter.ToUInt64(b, t) : BitConverter.ToUInt32(b, t);
        if (v == 0) break;
        ulong highBit = pe32plus ? 0x8000000000000000UL : 0x80000000UL;
        if ((v & highBit) == 0) {
          int hOff = toOff((int)(v & 0x7FFFFFFF));
          if (hOff > 0) im.Funcs.Add(ReadCString(b, hOff + 2));
        } else {
          im.Funcs.Add("#" + (v & 0xFFFF));
        }
        t += pe32plus ? 8 : 4;
      }
      list.Add(im);
      d += 20;
    }
    return list;
  }

  static string ReadCString(byte[] b, int off) {
    if (off < 0 || off >= b.Length) return "";
    int end = off;
    while (end < b.Length && b[end] != 0) end++;
    return Encoding.ASCII.GetString(b, off, end - off);
  }

  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern IntPtr LoadLibraryExW(string name, IntPtr file, uint flags);
  [DllImport("kernel32.dll", CharSet=CharSet.Ansi, SetLastError=true)]
  static extern IntPtr GetProcAddress(IntPtr mod, string name);
  [DllImport("kernel32.dll", SetLastError=true)]
  static extern bool FreeLibrary(IntPtr mod);

  public static string Verify(string path) {
    var sb = new StringBuilder();
    foreach (var im in Read(path)) {
      IntPtr mod = LoadLibraryExW(im.Dll, IntPtr.Zero, 0);
      if (mod == IntPtr.Zero) {
        sb.AppendLine("DLL LOAD FAILED: " + im.Dll + " err=" + Marshal.GetLastWin32Error());
        continue;
      }
      var missing = new List<string>();
      foreach (var f in im.Funcs) {
        if (!f.StartsWith("#") && GetProcAddress(mod, f) == IntPtr.Zero) missing.Add(f);
      }
      if (missing.Count > 0) sb.AppendLine("MISSING in " + im.Dll + ": " + string.Join(", ", missing));
      FreeLibrary(mod);
    }
    return sb.ToString();
  }
}
'@ -Language CSharp

foreach ($p in $Path) {
  $item = Get-Item $p
  Write-Output ("=== {0}  {1}  {2} bytes ===" -f $item.Name, $item.LastWriteTime.ToString('yyyy-MM-dd HH:mm'), $item.Length)
  $imports = [PeImports]::Read($item.FullName)
  Write-Output ("dlls({0}): {1}" -f $imports.Count, (($imports | ForEach-Object { $_.Dll }) -join ', '))
  $task = @()
  foreach ($im in $imports) { if ($im.Funcs -contains 'TaskDialogIndirect') { $task += $im.Dll } }
  Write-Output ("TaskDialogIndirect import: {0}" -f ($(if ($task) { $task -join '+' } else { 'none' })))
  if (-not $Quiet) {
    $report = [PeImports]::Verify($item.FullName)
    if ([string]::IsNullOrWhiteSpace($report)) { Write-Output 'unresolved imports: none' } else { Write-Output $report.Trim() }
  }
}

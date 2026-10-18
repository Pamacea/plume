Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type @'
using System;using System.Text;using System.Collections.Generic;using System.Runtime.InteropServices;
public class W2 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public struct RECT { public int L,T,R,B; }
  public static List<string> WindowsOf(uint target) {
    var res = new List<string>();
    EnumWindows((h,l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == target) {
        var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
        RECT r; GetWindowRect(h, out r);
        res.Add(string.Format("h=0x{0:X} vis={1} title='{2}' rect={3},{4} {5}x{6} ex=0x{7:X}",
          h.ToInt64(), IsWindowVisible(h), sb.ToString(), r.L, r.T, r.R-r.L, r.B-r.T, GetWindowLong(h,-20)));
      }
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
'@

$proc = Get-Process plume -ErrorAction SilentlyContinue
if (-not $proc) { 'plume NOT RUNNING'; exit 1 }
'plume PID=' + $proc.Id
[W2]::WindowsOf([uint32]$proc.Id) | ForEach-Object { 'avant: ' + $_ }

$p = Start-Process "$env:SystemRoot\System32\notepad.exe" -PassThru
for ($i=0; $i -lt 20 -and $p.MainWindowHandle -eq 0; $i++) { Start-Sleep -Milliseconds 250; $p.Refresh() }
if ($p.MainWindowHandle -eq 0) { 'pas de fenetre notepad'; exit 1 }
$src = New-Object -ComObject WScript.Shell
[void]$src.AppActivate($p.Id)
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait("c'est ")
Start-Sleep -Milliseconds 900

[W2]::WindowsOf([uint32]$proc.Id) | ForEach-Object { 'apres: ' + $_ }

$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$img=New-Object System.Drawing.Bitmap($b.Width,$b.Height)
$g=[System.Drawing.Graphics]::FromImage($img)
$g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size)
$img.Save('C:\Users\Yanis\Projects\-software\plume\test_screen3.png')
'capture OK'

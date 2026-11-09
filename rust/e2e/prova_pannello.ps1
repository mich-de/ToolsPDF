param([string]$Exe, [string]$Dir, [string]$Out)
$ErrorActionPreference = 'Continue'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System; using System.Threading; using System.Runtime.InteropServices;
public static class W {
 [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rt,B; }
 [DllImport("user32")] public static extern bool SetProcessDPIAware();
 [DllImport("user32")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32")] public static extern void mouse_event(uint f,int x,int y,int d,IntPtr e);
 [DllImport("user32",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
 [DllImport("user32")] public static extern bool GetWindowRect(IntPtr h,out R r);
 [DllImport("user32")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32")] public static extern uint GetDpiForWindow(IntPtr h);
 public static IntPtr ByClass(string c){ return FindWindow(c,null); }
 public static void Click(int x,int y){ SetCursorPos(x,y); Thread.Sleep(150); mouse_event(2,0,0,0,IntPtr.Zero); Thread.Sleep(80); mouse_event(4,0,0,0,IntPtr.Zero); Thread.Sleep(250); }
 public static void DragWithin(int x,int y0,int y1){ SetCursorPos(x,y0); Thread.Sleep(150); mouse_event(2,0,0,0,IntPtr.Zero); Thread.Sleep(150);
   for(int i=1;i<=20;i++){ SetCursorPos(x,y0+(y1-y0)*i/20); mouse_event(1,0,0,0,IntPtr.Zero); Thread.Sleep(25);} Thread.Sleep(200); mouse_event(4,0,0,0,IntPtr.Zero); Thread.Sleep(300); }
 public static void Drag(int x0,int y0,int x1,int y1){ new Thread(()=>{ Thread.Sleep(500); SetCursorPos(x0,y0); Thread.Sleep(150); mouse_event(2,0,0,0,IntPtr.Zero); Thread.Sleep(200);
   for(int i=1;i<=25;i++){ SetCursorPos(x0+(x1-x0)*i/25,y0+(y1-y0)*i/25); mouse_event(1,0,0,0,IntPtr.Zero); Thread.Sleep(20);}
   for(int i=0;i<20;i++){ mouse_event(1,0,0,0,IntPtr.Zero); Thread.Sleep(30);} mouse_event(4,0,0,0,IntPtr.Zero); }){IsBackground=true}.Start(); }
}
'@
[W]::SetProcessDPIAware() | Out-Null

function Shot($name, $r) {
  $w = $r.Rt - $r.L; $h = $r.B - $r.T
  $b = New-Object Drawing.Bitmap $w, $h; $g = [Drawing.Graphics]::FromImage($b)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object Drawing.Size $w, $h)); $b.Save((Join-Path $Out $name)); $g.Dispose(); $b.Dispose()
}
function Check([string]$label, $ok) { "{0}  {1}" -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $label }

function DragFiles([string[]]$paths, [int]$zone) {
  for ($try = 1; $try -le 4; $try++) {
    $script:f = New-Object Windows.Forms.Form; $script:f.StartPosition = 'Manual'; $script:f.Location = New-Object Drawing.Point 100, 100
    $script:f.Size = New-Object Drawing.Size 300, 200; $script:f.TopMost = $true; $script:f.Text = 'Sorgente prova'
    $script:list = New-Object Collections.Specialized.StringCollection; foreach ($p in $paths) { [void]$script:list.Add($p) }
    $script:tm = New-Object Windows.Forms.Timer; $script:tm.Interval = 12000; $script:tm.Add_Tick({ $script:f.Close() }); $script:tm.Start()
    $script:f.Add_MouseDown({ $d = New-Object Windows.Forms.DataObject; $d.SetFileDropList($script:list); $script:eff = $script:f.DoDragDrop($d, [Windows.Forms.DragDropEffects]::Copy); $script:f.Close() })
    $script:y = [int]($script:bar.T + (48 + $zone * 66 + 29) * $script:s); $script:x = [int](($script:bar.L + $script:bar.Rt) / 2)
    $script:f.Add_Shown({ $script:f.Activate(); [W]::Drag(250, 200, $script:x, $script:y) })
    $script:eff = 'nessun rilascio'
    [Windows.Forms.Application]::Run($script:f); $script:tm.Dispose()
    "INFO  rilascio: $script:eff"
    if ($script:eff -ne 'nessun rilascio') { return }
    Start-Sleep -Seconds 1
  }
}

$p = Start-Process $Exe -PassThru
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 150; $h = [W]::ByClass('ToolsPdfSidebar') }
if ($h -eq [IntPtr]::Zero) { 'NO WINDOW'; exit 1 }
Start-Sleep -Milliseconds 500
$r = New-Object W+R; [W]::GetWindowRect($h, [ref]$r) | Out-Null; $script:bar = $r
$script:s = [W]::GetDpiForWindow($h) / 96.0; $s = $script:s
Shot 'p-bar.png' $r
$panel = [W]::ByClass('ToolsPdfPanel')

# 1. Tre file su Unisci: il pannello si apre con l'ordine alfabetico.
DragFiles @("$Dir\uno.pdf", "$Dir\dispensa.pdf", "$Dir\relazione.docx") 3
Start-Sleep -Seconds 4
Check 'Pannello aperto dopo il rilascio' ([W]::IsWindowVisible($panel))
$pr = New-Object W+R; [W]::GetWindowRect($panel, [ref]$pr) | Out-Null
"pannello: $($pr.L),$($pr.T),$($pr.Rt),$($pr.B)"
Shot 'p-join1.png' $pr
$W = ($pr.Rt - $pr.L) / $s; $H = ($pr.B - $pr.T) / $s
function PX($lx) { [int]($pr.L + $lx * $s) }
function PY($ly) { [int]($pr.T + $ly * $s) }

# 2. Trascina la terza riga (uno.pdf) in cima, poi sposta la seconda in fondo con la freccia.
[W]::DragWithin((PX 150), (PY (48 + 2 * 62 + 31)), (PY (48 + 4)))
Shot 'p-join2.png' $pr
[W]::Click((PX ($W - 12 - 30 - 13)), (PY (48 + 1 * 62 + 31)))
Start-Sleep -Milliseconds 400
# Seleziona dispensa (ora terza) per vederne le pagine.
[W]::Click((PX 150), (PY (48 + 2 * 62 + 31)))
Start-Sleep -Milliseconds 600
Shot 'p-join3.png' $pr

# 3. Info e Impostazioni dalla barra.
[W]::Click([int]($r.L + 134 * $s), [int]($r.T + 24 * $s)); Start-Sleep -Milliseconds 600; Shot 'p-info.png' $pr
[W]::Click([int]($r.L + 164 * $s), [int]($r.T + 24 * $s)); Start-Sleep -Milliseconds 600; Shot 'p-settings.png' $pr
[W]::Click((PX 130), (PY 100)); Start-Sleep -Milliseconds 300
[W]::Click((PX 170), (PY 414)); Start-Sleep -Milliseconds 800
Shot 'p-settings-saved.png' $pr
Shot 'p-bar-2mb.png' $r
$ini = Join-Path (Split-Path $Exe) 'ToolsPDF.ini'
Check 'Impostazioni salvate accanto all''exe con 2 MB' ((Test-Path $ini) -and ((Get-Content $ini -Raw) -match 'limite_mb=2'))
# Torna a 1 MB.
[W]::Click((PX (12 + 39)), (PY 100)); Start-Sleep -Milliseconds 300
[W]::Click((PX 170), (PY 414)); Start-Sleep -Milliseconds 500
Check 'Ripristinato 1 MB' ((Get-Content $ini -Raw) -match 'limite_mb=1')

# 4. Di nuovo la lista e Unisci dal pannello.
[W]::Click([int]($r.L + 110 * $s), [int]($r.T + (48 + 3 * 66 + 20) * $s)); Start-Sleep -Milliseconds 600
Shot 'p-join4.png' $pr
[W]::Click((PX 89), (PY ($H - 30)))
$t = [Diagnostics.Stopwatch]::StartNew(); $out = "$Dir\uno_unito_1MB.pdf"
while ($t.Elapsed.TotalSeconds -lt 90 -and -not (Test-Path $out)) { Start-Sleep -Milliseconds 300 }
Check 'Unione creata' (Test-Path $out)
Check 'Pannello chiuso dopo Unisci' (-not [W]::IsWindowVisible($panel))
Start-Sleep -Seconds 1; Shot 'p-done.png' $r

[W]::PostMessage($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
for ($i = 0; $i -lt 60 -and -not $p.HasExited; $i++) { Start-Sleep -Milliseconds 100 }
Check 'Chiusura' $p.HasExited
if (-not $p.HasExited) { Stop-Process $p -Force }

param([string]$Exe, [string]$Dir, [string]$Out, [string]$TempDir)
$ErrorActionPreference = 'Continue'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing @'
using System; using System.Threading; using System.Text; using System.Collections.Generic;
using System.Runtime.InteropServices; using ComTypes = System.Runtime.InteropServices.ComTypes;
using FORMATETC = System.Runtime.InteropServices.ComTypes.FORMATETC;
using STGMEDIUM = System.Runtime.InteropServices.ComTypes.STGMEDIUM;

public static class W {
 [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rt,B; }
 [DllImport("user32")] public static extern bool SetProcessDPIAware();
 [DllImport("user32")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32")] public static extern void mouse_event(uint f,int x,int y,int d,IntPtr e);
 [DllImport("user32",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string t);
 [DllImport("user32",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p,IntPtr a,string c,string t);
 [DllImport("user32")] public static extern IntPtr GetDlgItem(IntPtr d,int id);
 [DllImport("user32")] public static extern bool GetWindowRect(IntPtr h,out R r);
 [DllImport("user32")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32")] public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32",CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,string l);
 [DllImport("user32")] public static extern bool SystemParametersInfo(uint a,uint b,out R r,uint c);
 [DllImport("user32")] public static extern uint GetDpiForWindow(IntPtr h);
 public delegate bool EP(IntPtr h,IntPtr l);
 [DllImport("user32")] public static extern bool EnumChildWindows(IntPtr p,EP e,IntPtr l);
 [DllImport("user32",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder s,int n);
 public static IntPtr ByClass(string c){ return FindWindow(c,null); }
 public static IntPtr Dialog(string title){ return FindWindow("#32770",title); }
 public static R Work(){ R r; SystemParametersInfo(0x30,0,out r,0); return r; }
 public static IntPtr FirstEdit(IntPtr d){ IntPtr f=IntPtr.Zero; EnumChildWindows(d,(h,l)=>{ var c=new StringBuilder(64); GetClassName(h,c,64); if(c.ToString()=="Edit"){ f=h; return false; } return true; },IntPtr.Zero); return f; }
 public static void FillDialog(IntPtr d,string path){ var e=FirstEdit(d); SendMessage(e,0x000C,IntPtr.Zero,path); Thread.Sleep(150); SendMessage(GetDlgItem(d,1),0xF5,IntPtr.Zero,IntPtr.Zero); }
 public static void ClickOk(IntPtr d){ SendMessage(GetDlgItem(d,2),0xF5,IntPtr.Zero,IntPtr.Zero); SendMessage(GetDlgItem(d,1),0xF5,IntPtr.Zero,IntPtr.Zero); }
 public static void Click(int x,int y){ SetCursorPos(x,y); Thread.Sleep(120); mouse_event(2,0,0,0,IntPtr.Zero); Thread.Sleep(60); mouse_event(4,0,0,0,IntPtr.Zero); }
 public static volatile bool Hovering;
 public static void Drag(int x0,int y0,int x1,int y1){ new Thread(()=>{ Thread.Sleep(500); SetCursorPos(x0,y0); Thread.Sleep(150); mouse_event(2,0,0,0,IntPtr.Zero); Thread.Sleep(200);
   for(int i=1;i<=25;i++){ SetCursorPos(x0+(x1-x0)*i/25,y0+(y1-y0)*i/25); mouse_event(1,0,0,0,IntPtr.Zero); Thread.Sleep(20);}
   Hovering=true; for(int i=0;i<30;i++){ mouse_event(1,0,0,0,IntPtr.Zero); Thread.Sleep(30);} Hovering=false; mouse_event(4,0,0,0,IntPtr.Zero); }){IsBackground=true}.Start(); }
}

[ComImport,Guid("00000121-0000-0000-C000-000000000046"),InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IDropSource { [PreserveSig] int QueryContinueDrag(int esc,int keys); [PreserveSig] int GiveFeedback(int effect); }
public class Source : IDropSource {
 public int QueryContinueDrag(int esc,int keys){ if(esc!=0) return 0x00040101; return (keys&1)==0 ? 0x00040100 : 0; }
 public int GiveFeedback(int effect){ return 0x00040102; }
}

/// Oggetto trascinato come fa Outlook: file virtuali, senza nessun percorso su disco.
[ComVisible(true)]
public class Virtual : ComTypes.IDataObject {
 string[] names; byte[][] contents; short fgd, fc;
 [DllImport("user32",CharSet=CharSet.Unicode)] static extern int RegisterClipboardFormat(string s);
 [DllImport("ole32")] static extern int CreateStreamOnHGlobal(IntPtr h,bool del,out IntPtr s);
 [DllImport("ole32")] static extern int DoDragDrop(ComTypes.IDataObject d,IDropSource s,int ok,out int eff);
 public Virtual(string[] n,byte[][] c){ names=n; contents=c; fgd=(short)RegisterClipboardFormat("FileGroupDescriptorW"); fc=(short)RegisterClipboardFormat("FileContents"); }
 public static Virtual One(string n,byte[] b){ return new Virtual(new string[]{n},new byte[][]{b}); }
 public int Run(){ int e; int r=DoDragDrop(this,new Source(),1,out e); LastEffect=e; return r; }
 public static int LastEffect;
 public void GetData(ref FORMATETC f,out STGMEDIUM m){
  m=new STGMEDIUM(); m.pUnkForRelease=null;
  if(f.cfFormat==fgd){
   byte[] b=new byte[4+592*names.Length]; BitConverter.GetBytes(names.Length).CopyTo(b,0);
   for(int i=0;i<names.Length;i++){ Encoding.Unicode.GetBytes(names[i]).CopyTo(b,4+592*i+72); }
   IntPtr h=Marshal.AllocHGlobal(b.Length); Marshal.Copy(b,0,h,b.Length); m.tymed=ComTypes.TYMED.TYMED_HGLOBAL; m.unionmember=h; return; }
  if(f.cfFormat==fc && f.lindex>=0 && f.lindex<contents.Length){
   byte[] c=contents[f.lindex]; IntPtr g=Marshal.AllocHGlobal(c.Length); Marshal.Copy(c,0,g,c.Length);
   IntPtr s; CreateStreamOnHGlobal(g,true,out s); m.tymed=ComTypes.TYMED.TYMED_ISTREAM; m.unionmember=s; return; }
  throw new COMException("formato",unchecked((int)0x80040064));
 }
 public void GetDataHere(ref FORMATETC f,ref STGMEDIUM m){ throw new NotImplementedException(); }
 public int QueryGetData(ref FORMATETC f){ return (f.cfFormat==fgd||f.cfFormat==fc)?0:unchecked((int)0x80040064); }
 public int GetCanonicalFormatEtc(ref FORMATETC a,out FORMATETC b){ b=a; return unchecked((int)0x80040130); }
 public void SetData(ref FORMATETC f,ref STGMEDIUM m,bool r){ throw new NotImplementedException(); }
 public ComTypes.IEnumFORMATETC EnumFormatEtc(ComTypes.DATADIR d){ throw new NotImplementedException(); }
 public int DAdvise(ref FORMATETC f,ComTypes.ADVF a,ComTypes.IAdviseSink s,out int c){ c=0; return unchecked((int)0x80040003); }
 public void DUnadvise(int c){ }
 public int EnumDAdvise(out ComTypes.IEnumSTATDATA e){ e=null; return unchecked((int)0x80040003); }
}
'@
[W]::SetProcessDPIAware() | Out-Null

function Shot($name, $r) {
  $w = $r.Rt - $r.L; $h = $r.B - $r.T
  $b = New-Object Drawing.Bitmap $w, $h; $g = [Drawing.Graphics]::FromImage($b)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object Drawing.Size $w, $h)); $b.Save((Join-Path $Out $name)); $g.Dispose(); $b.Dispose()
}

$script:scale = 1.0; $script:bar = $null
function ZoneY([int]$i) { [int]($script:bar.T + (44 + $i * 66 + 29) * $script:scale) }
function BarX { [int](($script:bar.L + $script:bar.Rt) / 2) }

# Trascinamento reale di file veri, con il cursore, su una zona.
function DragFiles([string[]]$paths, [int]$zone, [string]$hoverShot) {
  $script:f = New-Object Windows.Forms.Form; $script:f.StartPosition = 'Manual'; $script:f.Location = New-Object Drawing.Point 100, 100
  $script:f.Size = New-Object Drawing.Size 300, 200; $script:f.TopMost = $true; $script:f.Text = 'Sorgente prova'
  $script:list = New-Object Collections.Specialized.StringCollection; foreach ($p in $paths) { [void]$script:list.Add($p) }
  $script:tm = New-Object Windows.Forms.Timer; $script:tm.Interval = 15000; $script:tm.Add_Tick({ $script:f.Close() }); $script:tm.Start()
  $script:f.Add_MouseDown({ $d = New-Object Windows.Forms.DataObject; $d.SetFileDropList($script:list); $script:eff = $script:f.DoDragDrop($d, [Windows.Forms.DragDropEffects]::Copy); $script:f.Close() })
  $script:y = ZoneY $zone; $script:x = BarX
  $script:f.Add_Shown({ $script:f.Activate(); [W]::Drag(250, 200, $script:x, $script:y) })
  if ($hoverShot) {
    $script:t2 = New-Object Windows.Forms.Timer; $script:t2.Interval = 100; $script:hs = $hoverShot
    $script:t2.Add_Tick({ if ([W]::Hovering) { $script:t2.Stop(); Shot $script:hs $script:bar } }); $script:t2.Start()
  }
  $script:eff = "nessun rilascio"
  [Windows.Forms.Application]::Run($script:f)
  $script:tm.Dispose()
  "INFO  rilascio su zona $zone di $($paths -join ','): $script:eff"
}

# Trascinamento di un allegato virtuale, come da Outlook: nessun file su disco.
function DragVirtual([string]$name, [byte[]]$bytes, [int]$zone) {
  $script:f = New-Object Windows.Forms.Form; $script:f.StartPosition = 'Manual'; $script:f.Location = New-Object Drawing.Point 100, 100
  $script:f.Size = New-Object Drawing.Size 300, 200; $script:f.TopMost = $true; $script:f.Text = 'Outlook simulato'
  $script:tm = New-Object Windows.Forms.Timer; $script:tm.Interval = 15000; $script:tm.Add_Tick({ $script:f.Close() }); $script:tm.Start()
  $script:v = [Virtual]::One($name, $bytes)
  $script:f.Add_MouseDown({ $script:eff = [Virtual]::LastEffect; [void]$script:v.Run(); $script:eff = [Virtual]::LastEffect; $script:f.Close() })
  $script:y = ZoneY $zone; $script:x = BarX
  $script:f.Add_Shown({ $script:f.Activate(); [W]::Drag(250, 200, $script:x, $script:y) })
  [Windows.Forms.Application]::Run($script:f)
  $script:tm.Dispose()
}

function WaitFor([string]$pattern, [int]$seconds) {
  $t = [Diagnostics.Stopwatch]::StartNew()
  while ($t.Elapsed.TotalSeconds -lt $seconds) {
    if (Get-ChildItem $Dir -Filter $pattern -Force -ErrorAction SilentlyContinue) { return $true }
    Start-Sleep -Milliseconds 300
  }
  return $false
}

function AnswerSave([string]$path, [int]$seconds) {
  $t = [Diagnostics.Stopwatch]::StartNew()
  while ($t.Elapsed.TotalSeconds -lt $seconds) {
    $d = [W]::Dialog('Dove vuoi salvare il PDF?')
    if ($d -ne [IntPtr]::Zero) { Start-Sleep -Milliseconds 700; [W]::FillDialog($d, $path); return $true }
    Start-Sleep -Milliseconds 200
  }
  return $false
}

function Check([string]$label, $ok) { "{0}  {1}" -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $label }

$before = [W]::Work().Rt
$officeBefore = @(Get-Process WINWORD, EXCEL -ErrorAction SilentlyContinue).Count
$p = Start-Process $Exe -PassThru
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 150; $h = [W]::ByClass('ToolsPdfSidebar') }
if ($h -eq [IntPtr]::Zero) { 'NO WINDOW'; if (-not $p.HasExited) { Stop-Process $p }; exit 1 }
Start-Sleep -Milliseconds 400
$r = New-Object W+R; [W]::GetWindowRect($h, [ref]$r) | Out-Null; $script:bar = $r
$script:scale = [W]::GetDpiForWindow($h) / 96.0
"barra: $($r.L),$($r.T),$($r.Rt),$($r.B) scala $($script:scale) area di lavoro $before -> $([W]::Work().Rt)"
Shot 'e2e-empty.png' $r

# 4. Unisci in due trascinamenti diversi, poi il tasto.
DragFiles @("$Dir\uno.pdf") 3 $null
Start-Sleep -Seconds 2; Shot 'e2e-tray1.png' $r
DragFiles @("$Dir\relazione.docx") 3 $null
Start-Sleep -Seconds 1; Shot 'e2e-tray2.png' $r
$s = $script:scale
[W]::Click([int]($r.L + 63 * $s), [int]($r.T + 314 * $s))
Check 'Unisci con vassoio e tasto' (WaitFor 'uno_unito_1MB.pdf' 60)

# 5. Allegato virtuale come da Outlook: chiede dove salvare e rifiuta le temporanee.
$pdfBytes = [IO.File]::ReadAllBytes("$Dir\uno.pdf")
DragVirtual 'allegato_da_outlook.pdf' $pdfBytes 0
$tempTarget = Join-Path $TempDir 'non_qui.pdf'
Check 'Allegato virtuale: compare il dialogo' (AnswerSave $tempTarget 15)
$refused = $false
for ($i = 0; $i -lt 30 -and -not $refused; $i++) {
  Start-Sleep -Milliseconds 200
  $m = [W]::Dialog('Tools PDF')
  if ($m -ne [IntPtr]::Zero) { Start-Sleep -Milliseconds 300; $dr = New-Object W+R; [W]::GetWindowRect($m, [ref]$dr) | Out-Null; Shot 'e2e-refused.png' $dr; [W]::ClickOk($m); $refused = $true }
}
Check 'Cartella temporanea rifiutata' $refused
Check 'Il dialogo si riapre' (AnswerSave "$Dir\scelto_outlook.pdf" 15)
Check 'Allegato salvato dove scelto' (WaitFor 'scelto_outlook.pdf' 20)
Check 'Niente salvato in temporanea' (-not (Test-Path $tempTarget))

# 6. Allegato Word virtuale: copia nascosta nella cartella scelta, poi cancellata.
$docBytes = [IO.File]::ReadAllBytes("$Dir\relazione.docx")
DragVirtual 'allegato_word.docx' $docBytes 0
Check 'Word virtuale: compare il dialogo' (AnswerSave "$Dir\scelto_word.pdf" 15)
$sawWork = $false; $t = [Diagnostics.Stopwatch]::StartNew()
while ($t.Elapsed.TotalSeconds -lt 60 -and -not (Test-Path "$Dir\scelto_word.pdf")) {
  if (Get-ChildItem $Dir -Force -Filter '~toolspdf_*' -ErrorAction SilentlyContinue) { $sawWork = $true }
  Start-Sleep -Milliseconds 100
}
Check 'Word virtuale salvato' (Test-Path "$Dir\scelto_word.pdf")
"INFO  copia di lavoro vista nella cartella scelta: $sawWork"

Start-Sleep -Seconds 1; Shot 'e2e-done.png' $r

# 8. Tasto cartella sull'ultima riga completata.
$shell = New-Object -ComObject Shell.Application
$openBefore = @($shell.Windows() | ForEach-Object { $_.HWND })
$rowY = 44 + 3 * 66 + 58 + 36 + 26
[W]::Click([int]($r.Rt - 26 * $s), [int]($r.T + ($rowY + 14) * $s))
Start-Sleep -Seconds 3
$new = @($shell.Windows() | Where-Object { $openBefore -notcontains $_.HWND })
Check 'Tasto cartella apre Esplora' ($new.Count -ge 1)
foreach ($w in $new) { "INFO  aperta: $($w.LocationURL)"; $w.Quit() }

# Chiusura.
$c = [Diagnostics.Stopwatch]::StartNew(); [W]::PostMessage($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
for ($i = 0; $i -lt 100 -and -not $p.HasExited; $i++) { Start-Sleep -Milliseconds 100 }
Check "Chiusura ($($c.ElapsedMilliseconds) ms)" $p.HasExited
if (-not $p.HasExited) { Stop-Process $p -Force }
Check 'Spazio riservato liberato' ([W]::Work().Rt -eq $before)
Start-Sleep -Seconds 2
Check 'Office chiuso dopo le conversioni' (@(Get-Process WINWORD, EXCEL -ErrorAction SilentlyContinue).Count -le $officeBefore)
Check 'Nessun file di lavoro rimasto' (-not (Get-ChildItem $Dir -Force -Filter '~toolspdf_*' -ErrorAction SilentlyContinue))
'--- file prodotti'
Get-ChildItem $Dir -File -Force | Sort-Object Name | ForEach-Object { '{0,10} {1}' -f $_.Length, $_.Name }

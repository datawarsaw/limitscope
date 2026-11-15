# Guest-only, offline G6/G7 QA. Requires separate authorization for ONE fresh sandbox.
# No launcher included. Never run on the host. See docs/qa-g67-handoff.md.
# Guard precedes all output writes and the shutdown finally block.
if($env:USERNAME -cne 'WDAGUtilityAccount' -or $env:USERPROFILE -ine 'C:\Users\WDAGUtilityAccount'){throw 'WRONG_GUEST_IDENTITY'}
foreach($n in @('APPDATA','LOCALAPPDATA','TEMP','TMP')){if(-not([Environment]::GetEnvironmentVariable($n)).StartsWith('C:\Users\WDAGUtilityAccount\',[StringComparison]::OrdinalIgnoreCase)){throw 'NON_GUEST_ENVIRONMENT'}}
$env:HOME=$env:USERPROFILE
$ErrorActionPreference='Stop'
$out='C:\G67Results';$scratch='C:\G67Local';$inputRoot='C:\G67Input'
$r=[ordered]@{status='G67_BLOCKED';phase='PREFLIGHT';g7='NOT_TESTED';g6='NOT_TESTED';normal_user_qa='NOT_SATISFIED';install_attempts=@{'0.8.24-initial'=0;'0.8.25-upgrade'=0;'0.8.24-rollback'=0};app_launch_attempts=@{'0.8.24-initial'=0;'0.8.25-upgraded'=0;'0.8.24-rollback'=0};rollback_method='NOT_ATTEMPTED'}
function Save {$r.utc=[DateTime]::UtcNow.ToString('o');$r|ConvertTo-Json -Depth 18 | Set-Content -LiteralPath "$out\guest-result.json" -Encoding UTF8}
function Phase([string]$p){$r.phase=$p;Save}
function RunState {
 $k=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run',$false)
 $v=[ordered]@{LimitScope=$null;'Rate Limits'=$null};if($k){try{foreach($n in @('LimitScope','Rate Limits')){$v[$n]=$k.GetValue($n,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}}finally{$k.Dispose()}};return $v
}
# 2026-11-15 harness-only fix: the genuine 0.8.24 plugin wrote an UNQUOTED Run target (observed in results\old-after-ui-quit.json).
# Accept both quoted and unquoted Run values; stay strict on parsed target identity, exact '--hidden' argument, and guest-local existence.
function ParseRun([string]$run){
 $s=[string]$run;if($s){$s=$s.Trim()}
 if(-not $s){return $null}
 if($s[0] -eq '"'){$e=$s.IndexOf('"',1);if($e -lt 1){return $null};return [ordered]@{target=$s.Substring(1,$e-1);args=$s.Substring($e+1).Trim()}}
 $i=$s.IndexOf(' ');if($i -lt 0){return [ordered]@{target=$s;args=''}}
 return [ordered]@{target=$s.Substring(0,$i);args=$s.Substring($i+1).Trim()}
}
function RunAccepts([string]$run,[string]$exe){
 $p=ParseRun $run;if(-not $p){return $false}
 return ($p.target -ceq $exe -and $p.args -ceq '--hidden')
}
function RunMatches([string]$run,[string]$exe){
 $p=ParseRun $run;if(-not $p){return $false}
 return ((RunAccepts $run $exe) -and (Test-Path -LiteralPath $p.target))
}
function Arp {
 foreach($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser,[Microsoft.Win32.RegistryHive]::LocalMachine)){foreach($view in @([Microsoft.Win32.RegistryView]::Registry64,[Microsoft.Win32.RegistryView]::Registry32)){
  $b=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,$view);try{$k=$b.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Uninstall',$false);if($k){try{foreach($n in $k.GetSubKeyNames()){$s=$k.OpenSubKey($n,$false);try{if($s.GetValue('DisplayName') -in @('LimitScope','Rate Limits')){[ordered]@{hive=$hive.ToString();view=$view.ToString();key=$n;name=$s.GetValue('DisplayName');version=$s.GetValue('DisplayVersion');location=$s.GetValue('InstallLocation');uninstall=$s.GetValue('UninstallString')}}}finally{$s.Dispose()}}}finally{$k.Dispose()}}}finally{$b.Dispose()}
 }}
}
function Apps {@(Get-Process -Name LimitScope,rate-limits -ErrorAction SilentlyContinue)}
function Snapshot([string]$label){
 $rows=@(Arp | Sort-Object hive,key -Unique);$state=[ordered]@{utc=[DateTime]::UtcNow.ToString('o');arp=$rows;arp_count=$rows.Count;run=(RunState);processes=@(Get-CimInstance Win32_Process | Where-Object {$_.Name -match '^(LimitScope|rate-limits|uninstall.*|LimitScope_.*setup)(\.exe)?$'} | Select-Object Name,ProcessId,ParentProcessId,ExecutablePath,CommandLine)}
 $state|ConvertTo-Json -Depth 10 | Set-Content -LiteralPath "$out\$label.json" -Encoding UTF8;return $state
}
function Shot([string]$label){$b=[Windows.Forms.SystemInformation]::VirtualScreen;$m=New-Object Drawing.Bitmap $b.Width,$b.Height;$g=[Drawing.Graphics]::FromImage($m);try{$g.CopyFromScreen($b.Location,[Drawing.Point]::Empty,$b.Size);$m.Save("$out\$label.png",[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$m.Dispose()}}
function Installed([string]$ver){
 $a=@(Arp | Sort-Object hive,key -Unique);if($a.Count -ne 1 -or $a[0].version -cne $ver){throw "ARP_EXPECTED_ONE_$ver"}
 $dir=([string]$a[0].location).Trim('"');$exe=Join-Path $dir 'LimitScope.exe';$f=Get-Item -LiteralPath $exe
 if($f.VersionInfo.FileVersion -cne $ver -or $f.VersionInfo.ProductVersion -cne $ver){throw "INSTALLED_PE_MISMATCH_$ver"}
 $r.installed=[ordered]@{path=$exe;version=$ver;bytes=$f.Length;sha256=(Get-FileHash -LiteralPath $exe).Hash;arp=$a};Save;return $exe
}
function Install([string]$version,[string]$stage){
 Phase "INSTALL_$stage";if((Apps).Count){throw 'APP_RUNNING_BEFORE_INSTALL'}
 $r.install_attempts[$stage]++;Save
 $p=Start-Process -FilePath "$scratch\LimitScope_$($version)_x64-setup.exe" -ArgumentList '/S' -WorkingDirectory $scratch -PassThru
 $changes=New-Object 'Collections.Generic.List[object]';$last='';$until=[DateTime]::UtcNow.AddMinutes(3)
 do {$s=RunState;$json=$s|ConvertTo-Json -Compress;if($json -cne $last){$changes.Add(@{utc=[DateTime]::UtcNow.ToString('o');run=$s;installer_pid=$p.Id});$last=$json};Start-Sleep -Milliseconds 20;$p.Refresh()}while(-not $p.HasExited -and [DateTime]::UtcNow -lt $until)
 $changes.ToArray()|ConvertTo-Json -Depth 8|Set-Content -LiteralPath "$out\run-transitions-$stage.json" -Encoding UTF8
 if(-not $p.HasExited){throw "INSTALLER_TIMEOUT_$stage"};$r["installer_exit_$stage"]=$p.ExitCode;Save
 $null=Snapshot "installed-$stage";Shot "installed-$stage"
 if($p.ExitCode -ne 0){throw "INSTALLER_EXIT_$($p.ExitCode)_$stage"}
}
function LaunchWait([string]$exe,[string]$stage){
 if((Apps).Count){throw 'UNEXPECTED_APP_BEFORE_EXPLICIT_LAUNCH'}
 $r.app_launch_attempts[$stage]++;$r.phase="UI_$stage";Save
 $p=Start-Process -FilePath $exe -WorkingDirectory (Split-Path -Parent $exe) -PassThru
 $r["app_$stage"]=[ordered]@{pid=$p.Id;start_utc=$p.StartTime.ToUniversalTime().ToString('o');path=$exe};Save
 Start-Sleep -Seconds 12;Shot "ui-$stage-initial"
 $until=[DateTime]::UtcNow.AddMinutes(8)
 while(-not $p.HasExited -and [DateTime]::UtcNow -lt $until){Start-Sleep -Seconds 1;$p.Refresh()}
 if(-not $p.HasExited){throw "UI_TIMEOUT_$stage"};$r["app_exit_$stage"]=$p.ExitCode;Save
 if((Apps).Count){throw 'APP_PROCESSES_REMAIN'}
}
$seed=[ordered]@{
 'wallet-preferences-v1.json'='{"version":1,"monitored":{"openrouter":true,"deepseek":false}}'
 'provider-last-good-v1.json'=('{"version":1,"providers":[{"providerId":"zai","providerName":"Z.ai","account":{"label":"Account A","identity":"key:1234"},"limits":[{"label":"5-hour limit","usedPercent":42.5,"resetAt":"2026-09-29T16:00:00Z"}],"lastSuccessfulAt":"2026-09-29T11:55:00.000Z"}]}'.Replace('2026-09-29T11:55:00.000Z',([DateTime]::UtcNow.AddMinutes(-5).ToString('yyyy-MM-ddTHH:mm:ss.fffZ',[Globalization.CultureInfo]::InvariantCulture))).Replace('2026-09-29T16:00:00Z',([DateTime]::UtcNow.AddHours(4).ToString('yyyy-MM-ddTHH:mm:ssZ',[Globalization.CultureInfo]::InvariantCulture))))
 'usage-intelligence-v1.json'=('{"version":1,"enabled":true,"collectionStartedAt":1789292100000,"events":[],"cursors":[]}'.Replace('1789292100000',([DateTimeOffset]::UtcNow.AddMinutes(-5).ToUnixTimeMilliseconds().ToString([Globalization.CultureInfo]::InvariantCulture))))
 'quota-history-v1.json'=('{"version":2,"observations":[{"providerId":"zai","windowLabel":"5-hour","usedPercent":30.0,"observedAt":"2026-09-29T11:55:00.000Z","resetAt":"2026-09-29T16:00:00Z"}]}'.Replace('2026-09-29T11:55:00.000Z',([DateTime]::UtcNow.AddMinutes(-5).ToString('yyyy-MM-ddTHH:mm:ss.fffZ',[Globalization.CultureInfo]::InvariantCulture))).Replace('2026-09-29T16:00:00Z',([DateTime]::UtcNow.AddHours(4).ToString('yyyy-MM-ddTHH:mm:ssZ',[Globalization.CultureInfo]::InvariantCulture))))
}
function DataSnapshot([string]$label){
 $rows=@(foreach($name in $seed.Keys){$p=Join-Path $r.data_directory $name;if(Test-Path -LiteralPath $p){$f=Get-Item -LiteralPath $p;[ordered]@{name=$name;bytes=$f.Length;sha256=(Get-FileHash -LiteralPath $p).Hash;content=[IO.File]::ReadAllText($p)}}else{[ordered]@{name=$name;missing=$true}}})
 $sidecars=@(Get-ChildItem -LiteralPath $r.data_directory -File | Where-Object {$_.Name -match 'recovery|quarantin'} | Select-Object Name,Length)
 [ordered]@{utc=[DateTime]::UtcNow.ToString('o');directory=$r.data_directory;files=$rows;recovery_sidecars=$sidecars}|ConvertTo-Json -Depth 10|Set-Content -LiteralPath "$out\data-$label.json" -Encoding UTF8
 return ,$rows
}
try {
 if(-not(Test-Path -LiteralPath $out) -or @(Get-ChildItem -LiteralPath $out -Force).Count){throw 'RESULTS_NOT_EMPTY'}
 Add-Type -TypeDefinition @'
using System;using System.ComponentModel;using System.Runtime.InteropServices;using System.Security.Principal;
public static class G67Token {
 [DllImport("kernel32.dll")]static extern IntPtr GetCurrentProcess();
 [DllImport("kernel32.dll")]static extern bool CloseHandle(IntPtr h);
 [DllImport("advapi32.dll",SetLastError=true)]static extern bool OpenProcessToken(IntPtr p,uint a,out IntPtr t);
 [DllImport("advapi32.dll",SetLastError=true)]static extern bool GetTokenInformation(IntPtr t,int c,IntPtr b,int n,out int r);
 public static string Integrity(){IntPtr t=IntPtr.Zero,b=IntPtr.Zero;try{if(!OpenProcessToken(GetCurrentProcess(),8,out t))throw new Win32Exception();int n;GetTokenInformation(t,25,IntPtr.Zero,0,out n);if(n<=0)throw new Win32Exception();b=Marshal.AllocHGlobal(n);if(!GetTokenInformation(t,25,b,n,out n))throw new Win32Exception();return new SecurityIdentifier(Marshal.ReadIntPtr(b)).Value;}finally{if(b!=IntPtr.Zero)Marshal.FreeHGlobal(b);if(t!=IntPtr.Zero)CloseHandle(t);}}
}
'@
 $id=[Security.Principal.WindowsIdentity]::GetCurrent();$integrity=[G67Token]::Integrity()
 $who=@(& C:\Windows\System32\whoami.exe /all /fo csv);if($LASTEXITCODE){throw 'WHOAMI_FAILED'};$who|Set-Content -LiteralPath "$out\whoami-all.txt" -Encoding UTF8
 $sids=@([regex]::Matches(($who -join "`n"),'S-1-16-\d+')|ForEach-Object Value|Select-Object -Unique)
 $r.token=@{identity=$id.Name;user_sid=$id.User.Value;integrity_sid=$integrity;whoami_integrity=$sids;api='GetCurrentProcess/OpenProcessToken(TOKEN_QUERY)/GetTokenInformation(TokenIntegrityLevel=25)'};Save
 if($id.User.Value -eq 'S-1-5-18' -or $integrity -notin @('S-1-16-8192','S-1-16-12288') -or $sids.Count -ne 1 -or $sids[0] -cne $integrity){throw 'TOKEN_NOT_MEDIUM_OR_HIGH'}
 if($env:USERNAME -cne 'WDAGUtilityAccount' -or $env:USERPROFILE -ine 'C:\Users\WDAGUtilityAccount'){throw 'NOT_FRESH_SANDBOX_IDENTITY'}
 $net=@(Get-NetAdapter -IncludeHidden|Select-Object Name,Status,InterfaceDescription);$routes=@(Get-NetRoute|Where-Object DestinationPrefix -in @('0.0.0.0/0','::/0'))
 $r.isolation=@{computer=$env:COMPUTERNAME;network=$net;default_routes=$routes;drives=@(Get-PSDrive -PSProvider FileSystem|Select-Object Name,Root,DisplayRoot);map_metadata=@(Get-Item -LiteralPath $inputRoot,$out|Select-Object FullName,Attributes,LinkType,Target);runtime_mapping_enumeration='VSMB visibility limited; exact two maps verified in host XML'};Save
 if(@($net|Where-Object Status -notin @('Not Present','Disabled')).Count -or $routes.Count -or @(Get-PSDrive -PSProvider FileSystem|Where-Object DisplayRoot -like '\\*').Count){throw 'UNSAFE_ISOLATION'}
 if(Test-Path -LiteralPath $scratch){throw 'SCRATCH_PREEXISTS'};New-Item -ItemType Directory -Path $scratch | Out-Null
 $expected=@{'0.8.24'=@('CB5BBE5ADEA0D7FEF4DE8D123CDD23C88858C58E04258B71F7520084DE9BB796',4323639);'0.8.25'=@('8AE7ED458C977AC75D5484EBCDB7B373A3455687BCDEB41EA86AD0830428D049',4334249)}
 $r.artifacts=@(foreach($ver in @('0.8.24','0.8.25')){$name="LimitScope_$($ver)_x64-setup.exe";$src=Get-Item -LiteralPath "$inputRoot\$name";if((Get-FileHash -LiteralPath $src.FullName).Hash -cne $expected[$ver][0] -or $src.Length -ne $expected[$ver][1] -or $src.VersionInfo.FileVersion -cne $ver -or $src.VersionInfo.ProductVersion -cne $ver){throw 'MAPPED_INSTALLER_MISMATCH'};Copy-Item -LiteralPath $src.FullName -Destination "$scratch\$name";$f=Get-Item -LiteralPath "$scratch\$name";$hash=(Get-FileHash -LiteralPath $f.FullName).Hash;if($hash -cne $expected[$ver][0] -or $f.Length -ne $expected[$ver][1] -or $f.VersionInfo.FileVersion -cne $ver){throw 'LOCAL_INSTALLER_MISMATCH'};@{path=$f.FullName;sha256=$hash;bytes=$f.Length;version=$ver;mapped_hash_verified=$true}});Save
 Add-Type -AssemblyName System.Windows.Forms,System.Drawing
 $r.status='G67_PARTIAL';Install '0.8.24' '0.8.24-initial';$old=Installed '0.8.24';$r.old_executable=$r.installed
 LaunchWait $old '0.8.24-initial'
 $before=Snapshot 'old-after-ui-quit';Shot 'old-after-ui-quit'
 if(-not(RunMatches $before.run.LimitScope $old) -or $before.run.'Rate Limits'){throw 'OLD_UI_PLUGIN_AUTOSTART_NOT_ENABLED_EXACTLY_ONCE'}
 $r.old_autostart='ENABLED_BY_OBSERVED_GENUINE_UI_PLUGIN';Save
 $r.data_directory=Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'com.ratelimits.desktop'
 if(-not(Test-Path -LiteralPath $r.data_directory)){throw 'ACTUAL_APP_DATA_DIRECTORY_NOT_CREATED_BY_OLD_APP'}
 if((Get-Item -LiteralPath $r.data_directory).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'DATA_REPARSE'}
 Phase 'SYNTHETIC_SEED'
 foreach($name in $seed.Keys){[IO.File]::WriteAllText((Join-Path $r.data_directory $name),$seed[$name],(New-Object Text.UTF8Encoding($false)))}
 $r.seed_old_reader_before_upgrade='UNOBSERVED: seed written after the single initial old launch; verify actual old reader only after rollback, no extra launch';$null=DataSnapshot 'before-upgrade';Save
 Install '0.8.25' '0.8.25-upgrade'
 Phase 'G7_BEFORE_NEW_APP_LAUNCH';$g7=Snapshot 'G7-before-first-new-launch';$new=Installed '0.8.25';$null=DataSnapshot 'after-upgrade-before-launch'
 if($g7.arp_count -ne 1 -or -not(RunMatches $g7.run.LimitScope $new) -or $g7.run.'Rate Limits'){$r.g7='G7_FAIL';$r.g6='NOT_ATTEMPTED_G7_FAILED_NO_REPAIR';throw 'G7_AUTOSTART_OR_ARP_FAILED_STOP_NO_REGISTRY_REPAIR'}
 $r.g7='G7_PASS';Save
 LaunchWait $new '0.8.25-upgraded';$afterNew=DataSnapshot 'after-new-reader-writer'
 if(@(Get-ChildItem -LiteralPath $r.data_directory -File|Where-Object Name -match 'recovery|quarantin').Count){$r.g6='FAIL_HEALTHY_DATA_RECOVERY';throw 'HEALTHY_SEED_QUARANTINED'}
 foreach($x in $afterNew){if($x.missing){$r.g6='FAIL_DATA_MISSING';throw 'DATA_LOSS_AFTER_NEW_READER'}}
 $r.new_writer='UNCONFIRMED_UNLESS_OBSERVED_UI_AND_FILE_EVIDENCE';Save
 $r.rollback_method='DIRECT_DOWNGRADE_ATTEMPT';Install '0.8.24' '0.8.24-rollback'
 $old=Installed '0.8.24';$null=DataSnapshot 'after-downgrade-before-old-launch'
 LaunchWait $old '0.8.24-rollback';$null=DataSnapshot 'after-old-reader';$null=Snapshot 'rollback-final';Shot 'rollback-final'
 $r.g6='G6_PARTIAL_REQUIRES_ACTUAL_READER_UI_AND_NEW_WRITER_OBSERVATION';$r.status='G67_PARTIAL';Phase 'FINISHED'
}catch{
 $r.error=$_.Exception.Message;$r.phase_at_error=$r.phase;$r.phase='ABORTED';try{Save;if('System.Windows.Forms.SystemInformation' -as [type]){Shot 'abort'}}catch{}
}finally{
 $r.shutdown_requested_utc=[DateTime]::UtcNow.ToString('o');try{Save}catch{}
 & C:\Windows\System32\shutdown.exe /s /t 5 /c 'Single authorized G6 G7 synthetic QA completed; no release acceptance.' 2>&1 | Set-Content -LiteralPath "$out\shutdown.txt" -Encoding UTF8
}

# Sets up a Meta Portal for Porchlight over USB on Windows: the same steps as setup.sh (see its header for what
# and why). Double-click Setup-Portal.bat instead of running this by hand; Restore-Portal.bat runs it with
# -Restore, which undoes the system changes (not the app itself).
#
# Options: -Restore   undo the system changes
#          -Yes       answer every question with its default (no prompts)
# Environment: ANDROID_SERIAL picks the device when more than one is connected (standard adb behaviour);
#              PORCHLIGHT_ANY_DEVICE=1 skips the "is this a Portal?" question (for testing on an emulator).

param([switch]$Restore, [switch]$Yes)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with its progress bar on
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptDir

$Pkg = 'dev.porchlight.app'
$Activity = "$Pkg/.MainActivity"
$A11yService = "$Pkg/$Pkg.CallWakeUpAccessibilityService"
$ApkUrl = 'https://github.com/marcuscramer/porchlight/releases/latest/download/app-release.apk'
$State = '/data/local/tmp/porchlight-setup.state'
$VerifierPkg = 'com.facebook.appverifier'
$OtaPkgs = @('com.facebook.aloha.otaui', 'com.facebook.aloha.alohaotasetup')
$OverlayPkgs = @('com.facebook.aloha.rro.niu.android', 'com.facebook.aloha.rro.niu.settings')

# ----- output -----------------------------------------------------------------
function Step($t) { Write-Host ''; Write-Host "==> $t" -ForegroundColor White }
function Ok($t)   { Write-Host "  + $t" -ForegroundColor Green }
function Skip($t) { Write-Host "  - $t" -ForegroundColor Yellow }
function Warn($t) { Write-Host "  ! $t" -ForegroundColor Yellow }
function Die($t)  { Write-Host ''; Write-Host "Stopped: $t" -ForegroundColor Red; exit 1 }
function Ask($question, $default) {
  if ($Yes) { return $default -eq 'y' }
  $hint = if ($default -eq 'y') { 'Y/n' } else { 'y/N' }
  $answer = (Read-Host "  $question [$hint]").Trim().ToLower()
  if ($answer -eq '') { $answer = $default }
  return @('y', 'yes', 'j', 'ja') -contains $answer
}

# ----- adb: already here, on PATH, or downloaded from Google -----------------
function Resolve-Adb {
  $local = Join-Path $ScriptDir 'platform-tools\adb.exe'
  if (Test-Path $local) { return $local }
  $onPath = Get-Command adb -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  Step "Getting adb (Google's Android platform-tools) - into this folder only, nothing is installed system-wide"
  $zip = Join-Path $ScriptDir 'platform-tools.zip'
  try {
    Invoke-WebRequest -UseBasicParsing -Uri 'https://dl.google.com/android/repository/platform-tools-latest-windows.zip' -OutFile $zip
  } catch { Die "Couldn't download platform-tools. Check the internet connection and run again." }
  try { Expand-Archive -Path $zip -DestinationPath $ScriptDir -Force } catch { Die "Couldn't unpack platform-tools." }
  Remove-Item $zip -Force
  if (-not (Test-Path $local)) { Die 'adb is missing after the download.' }
  Ok 'adb ready'
  return $local
}

$script:Adb = $null
# adb with its exit status ignored (callers read the output); stderr folded in, CRs dropped. 'Continue' here:
# under 'Stop', Windows PowerShell 5.1 turns any stderr line of a native program (adb prints "daemon started"
# there) into a terminating error.
function A {
  $ErrorActionPreference = 'Continue'
  $out = & $script:Adb @args 2>&1 | ForEach-Object { "$_" }
  return ($out -join "`n") -replace "`r", ''
}
function Sh { return (A shell @args).Trim() }

# ----- wait for exactly one authorised device ---------------------------------
function Wait-ForDevice {
  Step 'Looking for the Portal'
  A start-server | Out-Null
  $saidNone = $false; $saidUnauth = $false
  while ($true) {
    $lines = @((A devices) -split "`n" | Select-Object -Skip 1 | Where-Object { $_.Trim() -ne '' })
    if ($env:ANDROID_SERIAL) { $lines = @($lines | Where-Object { $_ -like "$($env:ANDROID_SERIAL)`t*" }) }
    if ($lines.Count -gt 1) {
      $lines | ForEach-Object { Write-Host "    $_" }
      Die 'More than one device is connected. Unplug the others, or set ANDROID_SERIAL to the serial from the list above and run again.'
    }
    if ($lines.Count -eq 1 -and $lines[0] -match "`tdevice$") { Ok "Connected: $(($lines[0] -split "`t")[0])"; return }
    if ($lines.Count -eq 1 -and $lines[0] -match "`tunauthorized$") {
      if (-not $saidUnauth) { Warn 'On the Portal''s screen: allow USB debugging (tick "Always allow", then Allow).'; $saidUnauth = $true }
    } elseif (-not $saidNone) {
      Warn 'No Portal found yet. Check that:'
      Write-Host "      - it's connected to this computer with a USB-C cable (one that carries data, not only power)"
      Write-Host '      - Settings > Debug > ADB Enabled is switched on on the Portal'
      Write-Host '      - on Windows, the Portal may need a USB driver: Google''s USB driver usually works if Windows'
      Write-Host '        doesn''t recognise it on its own'
      $saidNone = $true
    }
    Start-Sleep -Seconds 2
  }
}

function Describe-Device {
  $script:Sdk = [int](Sh getprop ro.build.version.sdk)
  $device = Sh getprop ro.product.device
  $models = @{ ripley = 'Portal TV'; aloha = 'Portal+ (2018)'; ohana = 'Portal (2018)'; omni = 'Portal Mini / Portal (2019)';
               atlas = 'Portal (2019)'; cipher = 'Portal+ (2021)'; terry = 'Portal Go' }
  if ($models.ContainsKey($device)) {
    Ok "$($models[$device]) (Android API $script:Sdk)"
  } else {
    Warn "This doesn't look like a Meta Portal (device `"$device`", Android API $script:Sdk)."
    if ($env:PORCHLIGHT_ANY_DEVICE -ne '1' -and -not (Ask 'Continue anyway?' 'n')) { Die 'Nothing was changed.' }
  }
}

function Has-Pkg($p) { return @((Sh pm list packages $p) -split "`n" | Where-Object { $_.Trim() -eq "package:$p" }).Count -gt 0 }
function Pkg-Enabled($p) { return @((Sh pm list packages -d $p) -split "`n" | Where-Object { $_.Trim() -eq "package:$p" }).Count -eq 0 }
# See setup.sh: "null" means the setting doesn't exist, an empty answer is a failed read - retry, never act on it.
function Setting-Get($ns, $key) {
  for ($i = 0; $i -lt 5; $i++) {
    $v = Sh settings get $ns $key
    if ($v -ne '') { return $v }
    Start-Sleep -Seconds 1
  }
  Die "Couldn't read the Portal's setting $ns $key. Nothing more was changed; run the setup again."
}
# The value a setting had before this setup first changed it; recorded once, on the Portal (see setup.sh).
function Record-Once($k, $v) { Sh "grep -q '^$k=' $State 2>/dev/null || echo '$k=$v' >> $State" | Out-Null }
function Recorded($k) { return Sh "grep '^$k=' $State 2>/dev/null | head -1 | cut -d= -f2-" }
function Forget($k) { Sh "[ -f $State ] && grep -v '^$k=' $State > $State.tmp; mv $State.tmp $State 2>/dev/null" | Out-Null }

# ----- setup steps -------------------------------------------------------------
function Turn-OffInstallVerifier {
  Step 'Allowing Porchlight to update itself'
  if (Has-Pkg $VerifierPkg) {
    if (Pkg-Enabled $VerifierPkg) {
      Record-Once 'verifier_app' 'enabled'
      Sh pm disable-user --user 0 $VerifierPkg | Out-Null; Ok "Meta's install verifier switched off"
    } else { Ok "Meta's install verifier is already off" }
  } else { Skip 'No Meta install verifier on this device' }
  $current = Setting-Get global package_verifier_enable
  if ($current -ne '0') {
    Record-Once 'package_verifier_enable' $current
    Sh settings put global package_verifier_enable 0 | Out-Null; Ok "Android's install verification switched off"
  } else { Ok "Android's install verification is already off" }
}

function Install-App {
  Step 'Downloading the latest Porchlight'
  $apk = Join-Path $ScriptDir 'porchlight-latest.apk'
  try { Invoke-WebRequest -UseBasicParsing -Uri $ApkUrl -OutFile $apk }
  catch { Die "Couldn't download Porchlight. Check the internet connection and run again." }
  $head = [System.IO.File]::ReadAllBytes($apk)[0..1]
  if (-not ($head[0] -eq 0x50 -and $head[1] -eq 0x4B)) { Remove-Item $apk -Force; Die "The download isn't an app package. Try again later." }
  Ok 'Downloaded'
  Step 'Installing Porchlight on the Portal'
  $out = A install -r -g --no-streaming $apk
  Remove-Item $apk -Force
  if ($out -match 'Success') { Ok 'Installed (contacts and settings of an earlier install are kept)' }
  elseif ($out -match 'INSTALL_FAILED_VERSION_DOWNGRADE') { Ok 'A newer Porchlight is already installed - keeping it' }
  elseif ($out -match 'INSTALL_FAILED_UPDATE_INCOMPATIBLE') {
    Die "The Porchlight on this Portal was signed differently (a test build?). Uninstalling it deletes its contacts; if that's fine, run:  adb uninstall $Pkg  and then this setup again."
  } else { Write-Host $out; Die "The install didn't succeed (see above)." }
}

function Grant-Permissions {
  Step 'Camera and microphone'
  Sh pm grant $Pkg android.permission.CAMERA | Out-Null
  Sh pm grant $Pkg android.permission.RECORD_AUDIO | Out-Null
  if ($script:Sdk -ge 33) { Sh pm grant $Pkg android.permission.POST_NOTIFICATIONS | Out-Null }
  Ok "Allowed, so the first call doesn't stop at a permission question"
  Sh appops set $Pkg REQUEST_INSTALL_PACKAGES allow | Out-Null; Ok 'Allowed to install its own updates'
}

function Enable-CallWakeUp {
  Step 'Call wake-up (ends the screensaver and turns the TV on for a call)'
  $list = Setting-Get secure enabled_accessibility_services
  if ((":${list}:").Contains(":${A11yService}:")) { Ok 'Already on' }
  else {
    $new = if ($list -eq 'null') { $A11yService } else { "$($list):$A11yService" }
    Sh settings put secure enabled_accessibility_services "'$new'" | Out-Null
    Ok "Switched on (it can be turned off any time in Porchlight's Settings)"
  }
  Sh settings put secure accessibility_enabled 1 | Out-Null
  Sh appops set $Pkg SYSTEM_ALERT_WINDOW allow | Out-Null; Ok 'Allowed to cover the screen briefly while it does that'
}

function Fix-DialogOverlays {
  if ($script:Sdk -ge 29) { return }
  Step 'Making system dialogs readable'
  $overlays = Sh cmd overlay list
  foreach ($o in $OverlayPkgs) {
    if ($overlays.Contains("[x] $o")) {
      Record-Once "overlay_$o" 'enabled'
      Sh cmd overlay disable --user 0 $o | Out-Null; Ok "$o switched off"
    } elseif ($overlays.Contains(" $o")) { Ok "$o is already off" }
    else { Skip "$o isn't on this device" }
  }
}

function Maybe-StopMetaUpdates {
  $present = @($OtaPkgs | Where-Object { Has-Pkg $_ })
  if ($present.Count -eq 0) { return }
  Step "Meta's system updates"
  Write-Host '    Meta no longer develops the Portal; an update from Meta could still change how the Portal behaves.'
  Write-Host '    Switching its updater off keeps the Portal as it is now. Undo with Restore-Portal.'
  if (Ask "Switch Meta's system updates off?" 'n') {
    foreach ($p in $present) {
      if (Pkg-Enabled $p) { Record-Once "ota_$p" 'enabled'; Sh pm disable-user --user 0 $p | Out-Null }
    }
    Ok "Meta's system updates switched off"
  } else { Skip 'Left as they are' }
}

function Start-Porchlight {
  Step 'Starting Porchlight'
  Sh am start -n $Activity | Out-Null; Ok 'Porchlight is open on the Portal'
}

# ----- restore -----------------------------------------------------------------
function Restore-Portal {
  Step "Undoing Porchlight's system changes (the app itself stays installed)"
  $list = Setting-Get secure enabled_accessibility_services
  $new = (@($list -split ':' | Where-Object { $_ -ne '' -and $_ -ne $A11yService }) -join ':')
  if ($new -ne $list) {
    if ($new -ne '') { Sh settings put secure enabled_accessibility_services "'$new'" | Out-Null }
    else { Sh settings delete secure enabled_accessibility_services | Out-Null }
    Ok 'Call wake-up switched off'
  }
  if (Has-Pkg $Pkg) {
    Sh appops set $Pkg SYSTEM_ALERT_WINDOW default | Out-Null
    Sh appops set $Pkg REQUEST_INSTALL_PACKAGES default | Out-Null
    Ok 'Screen cover and self-update permissions taken back'
  }
  if ((Recorded 'verifier_app') -eq 'enabled') { Sh pm enable $VerifierPkg | Out-Null; Ok "Meta's install verifier switched back on"; Forget 'verifier_app' }
  $v = Recorded 'package_verifier_enable'
  if ($v -ne '') {
    if ($v -eq 'null') { Sh settings delete global package_verifier_enable | Out-Null }
    else { Sh settings put global package_verifier_enable $v | Out-Null }
    Ok "Android's install verification put back"; Forget 'package_verifier_enable'
  }
  foreach ($o in $OverlayPkgs) {
    if ((Recorded "overlay_$o") -eq 'enabled') { Sh cmd overlay enable --user 0 $o | Out-Null; Ok "$o switched back on"; Forget "overlay_$o" }
  }
  foreach ($p in $OtaPkgs) {
    if ((Recorded "ota_$p") -eq 'enabled') { Sh pm enable $p | Out-Null; Ok "$p switched back on"; Forget "ota_$p" }
  }
  if ((Has-Pkg $Pkg) -and (Ask 'Also uninstall Porchlight? This deletes its contacts.' 'n')) {
    A uninstall $Pkg | Out-Null; Ok 'Porchlight uninstalled'
  }
}

# ----- main --------------------------------------------------------------------
Write-Host 'Porchlight setup for Meta Portal' -ForegroundColor White
$script:Adb = Resolve-Adb
Wait-ForDevice
Describe-Device
if ($Restore) { Restore-Portal; Step 'Done.'; exit 0 }
Turn-OffInstallVerifier
Install-App
Grant-Permissions
Enable-CallWakeUp
Fix-DialogOverlays
Maybe-StopMetaUpdates
Start-Porchlight
Step 'Done.'
Write-Host '    Next, on the Portal: pick a name for it, then pair with someone (Add contact).'
Write-Host "    If this Portal is only for calls (e.g. for a relative), switch on Kiosk mode in Porchlight's Settings:"
Write-Host '    Porchlight then comes back by itself after a restart and after the screensaver.'

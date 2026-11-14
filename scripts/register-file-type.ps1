# Language: en-GB
[CmdletBinding()]
param([Parameter(Mandatory)][string]$ApplicationPath)
$ErrorActionPreference = 'Stop'
$application = (Get-Item -LiteralPath $ApplicationPath).FullName
if ([IO.Path]::GetFileName($application) -ne 'label-studio-420p.exe') { throw 'Choose the installed 420P Label Studio executable.' }
$iconSource = Join-Path (Split-Path $PSScriptRoot -Parent) 'assets/label-document-420p.ico'
if (-not (Test-Path -LiteralPath $iconSource -PathType Leaf)) { throw 'The project document icon is missing.' }
$progId = '420PLabelStudio.Project'
$choice = Get-ItemProperty -LiteralPath 'Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.420p\UserChoice' -ErrorAction SilentlyContinue
if ($choice -and $choice.ProgId -ne $progId) { throw 'Windows has a protected default for .420p. Select 420P Label Studio through Open with before registering; this script will not alter a protected UserChoice.' }
$state = Join-Path $env:LOCALAPPDATA '420P Label Studio/file-association'
[IO.Directory]::CreateDirectory($state) | Out-Null
$icon = Join-Path $state 'label-document-420p.ico'
$backup = Join-Path $state 'rollback.json'
$roots = @('Software\Classes\.420p', "Software\Classes\$progId")
if (-not (Test-Path -LiteralPath $backup)) {
    $records = foreach ($root in $roots) {
        $base = Get-Item -LiteralPath "Registry::HKEY_CURRENT_USER\$root" -ErrorAction SilentlyContinue
        if (-not $base) { [pscustomobject]@{path=$root; exists=$false; values=@()} ; continue }
        foreach ($key in @($base) + @(Get-ChildItem -LiteralPath $base.PSPath -Recurse)) {
            $values = foreach ($name in $key.GetValueNames()) { [pscustomobject]@{name=$name; kind=$key.GetValueKind($name).ToString(); value=$key.GetValue($name,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)} }
            [pscustomobject]@{path=$key.Name.Substring('HKEY_CURRENT_USER\'.Length); exists=$true; values=@($values)}
        }
    }
    [pscustomobject]@{language='en-GB'; registry=@($records); previousIcon=if(Test-Path -LiteralPath $icon){[Convert]::ToBase64String([IO.File]::ReadAllBytes($icon))}else{$null}} | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath $backup -Encoding utf8
}
Copy-Item -LiteralPath $iconSource -Destination $icon -Force
function Set-AssociationValue([string]$Subkey, [string]$Value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Subkey)
    try { $key.SetValue('', $Value, [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
}
Set-AssociationValue 'Software\Classes\.420p' $progId
Set-AssociationValue "Software\Classes\$progId" '420P Label Studio label'
Set-AssociationValue "Software\Classes\$progId\DefaultIcon" ('"' + $icon + '",0')
Set-AssociationValue "Software\Classes\$progId\shell" 'open'
Set-AssociationValue "Software\Classes\$progId\shell\open\command" ('"' + $application + '" "%1"')
$openWith = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Classes\.420p\OpenWithProgids')
try { $openWith.SetValue($progId, [byte[]]@(), [Microsoft.Win32.RegistryValueKind]::None) } finally { $openWith.Dispose() }
if (-not ('LabelStudioAssociationNative' -as [type])) {
    Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class LabelStudioAssociationNative {
    [DllImport("shell32.dll")] public static extern void SHChangeNotify(uint e, uint f, IntPtr a, IntPtr b);
    [DllImport("shlwapi.dll", CharSet=CharSet.Unicode)] public static extern int AssocQueryStringW(uint flags, uint type, string association, string extra, StringBuilder result, ref uint count);
}
'@
}
[LabelStudioAssociationNative]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
$length = [uint32]32768; $result = [Text.StringBuilder]::new([int]$length)
$status = [LabelStudioAssociationNative]::AssocQueryStringW(0, 2, '.420p', 'open', $result, [ref]$length)
if ($status -ne 0 -or $result.ToString() -ne $application) { throw 'Registration was written, but Windows has not resolved the intended default. Use Windows Open with to select the app.' }
if ((Get-FileHash -LiteralPath $icon).Hash -ne (Get-FileHash -LiteralPath $iconSource).Hash) { throw 'Installed document icon verification failed.' }
Write-Output 'Registered .420p with 420P Label Studio and verified the Windows open command. No .dymo keys were written.'

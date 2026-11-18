# Language: en-GB
[CmdletBinding()]
param([Parameter(Mandatory)][string]$ApplicationPath)
$ErrorActionPreference = 'Stop'
$application = (Get-Item -LiteralPath $ApplicationPath).FullName
if ([IO.Path]::GetFileName($application) -ne 'label-studio-420p.exe') { throw 'Choose the installed 420P Label Studio executable.' }
$progId = '420PLabelStudio.Project'
$choice = Get-ItemProperty -LiteralPath 'Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.420p\UserChoice' -ErrorAction SilentlyContinue
$protectedDefault = $choice -and $choice.ProgId -ne $progId
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
function Set-AssociationValue([string]$Subkey, [string]$Value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Subkey)
    try { $key.SetValue('', $Value, [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
}
if (-not $protectedDefault) { Set-AssociationValue 'Software\Classes\.420p' $progId }
Set-AssociationValue "Software\Classes\$progId" '420P Label Studio label'
Set-AssociationValue "Software\Classes\$progId\DefaultIcon" ('"' + $application + '",-2')
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
if ($protectedDefault) {
    Write-Output '420P Label Studio is available in Open with. To make it the default, choose it in Windows Open with.'
    return
}
$length = [uint32]32768; $result = [Text.StringBuilder]::new([int]$length)
$status = [LabelStudioAssociationNative]::AssocQueryStringW(0, 2, '.420p', 'open', $result, [ref]$length)
if ($status -ne 0 -or $result.ToString() -ne $application) { throw 'Registration was written, but Windows has not resolved the intended default. Use Windows Open with to select the app.' }
Write-Output 'Registered .420p with 420P Label Studio and verified the Windows open command. No .dymo keys were written.'

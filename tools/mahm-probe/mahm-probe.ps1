# tools/mahm-probe/mahm-probe.ps1 -- verify the Afterburner data path without a GUI.
#
# Opens Afterburner's MAHM shared memory and prints the published monitoring
# entries. This is how we confirm that HeartRate.dll is loaded, that its data
# source was enabled, and what value it reports -- all without a GUI.
#
# Layout comes from <Afterburner>\SDK\Include\MAHMSharedMemory.h.
# The header's bitness-dependent ``time_t`` field is why we read dwHeaderSize
# and dwEntrySize out of the header instead of hardcoding offsets.
param([string]$Filter = '')

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Map {
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Ansi)]
    public static extern IntPtr OpenFileMapping(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr MapViewOfFile(IntPtr h, uint access, uint offHi, uint offLo, UIntPtr bytes);
    [DllImport("kernel32.dll")]
    public static extern bool UnmapViewOfFile(IntPtr p);
    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr h);
}
"@

$FILE_MAP_READ = 0x0004
$MAX_PATH = 260

function Get-Bytes([IntPtr]$base, [int]$off, [int]$len) {
    $buf = New-Object byte[] $len
    [Runtime.InteropServices.Marshal]::Copy([IntPtr]::new($base.ToInt64() + $off), $buf, 0, $len)
    , $buf
}

function Get-Str([IntPtr]$base, [int]$off) {
    $buf = Get-Bytes $base $off $MAX_PATH
    $z = [Array]::IndexOf($buf, [byte]0)
    if ($z -lt 0) { $z = $MAX_PATH }
    [System.Text.Encoding]::ASCII.GetString($buf, 0, $z)
}

function Get-F32([IntPtr]$base, [int]$off) {
    [BitConverter]::ToSingle((Get-Bytes $base $off 4), 0)
}

function Get-U32([IntPtr]$base, [int]$off) {
    [BitConverter]::ToUInt32((Get-Bytes $base $off 4), 0)
}

$h = [Map]::OpenFileMapping($FILE_MAP_READ, $false, 'MAHMSharedMemory')
if ($h -eq [IntPtr]::Zero) {
    Write-Host "MAHMSharedMemory not open (err $([Runtime.InteropServices.Marshal]::GetLastWin32Error())) -- is Afterburner running?"
    exit 1
}

$p = [Map]::MapViewOfFile($h, $FILE_MAP_READ, 0, 0, [UIntPtr]::Zero)
if ($p -eq [IntPtr]::Zero) {
    [void][Map]::CloseHandle($h)
    Write-Host "MapViewOfFile failed (err $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
    exit 1
}

try {
    $sig     = Get-U32 $p 0
    $ver     = Get-U32 $p 4
    $hdrSize = Get-U32 $p 8
    $num     = Get-U32 $p 12
    $entSize = Get-U32 $p 16

    Write-Host ("signature=0x{0:X8}  version=0x{1:X8}  headerSize={2}  entries={3}  entrySize={4}" -f `
                $sig, $ver, $hdrSize, $num, $entSize)
    Write-Host ''

    # entry field offsets, from MAHM_SHARED_MEMORY_ENTRY
    for ($i = 0; $i -lt $num; $i++) {
        $b    = $hdrSize + $i * $entSize
        $name = Get-Str $p ($b + 0)
        if ($Filter -and ($name -notlike "*$Filter*")) { continue }

        $units = Get-Str $p ($b + $MAX_PATH)
        $fmt   = Get-Str $p ($b + 4 * $MAX_PATH)
        $data  = Get-F32 $p ($b + 5 * $MAX_PATH)
        $flags = Get-U32 $p ($b + 5 * $MAX_PATH + 12)
        $gpu   = Get-U32 $p ($b + 5 * $MAX_PATH + 16)
        $srcId = Get-U32 $p ($b + 5 * $MAX_PATH + 20)

        $osd = if ($flags -band 1) { 'OSD' } else { '-' }
        $lcd = if ($flags -band 2) { 'LCD' } else { '-' }
        $try = if ($flags -band 4) { 'tray' } else { '-' }

        $val = if ([single]::IsPositiveInfinity($data) -or $data -eq [single]::MaxValue) {
            'UNAVAILABLE (FLT_MAX)'
        } else {
            ('{0:0.###}' -f $data)
        }

        Write-Host ("[{0,3}] {1}" -f $i, $name)
        Write-Host ("      units='{0}'  format='{1}'  value={2}  flags={3}/{4}/{5}  gpu=0x{6:X}  srcId=0x{7:X2}" -f `
                    $units, $fmt, $val, $osd, $lcd, $try, $gpu, $srcId)
    }
} finally {
    [void][Map]::UnmapViewOfFile($p)
    [void][Map]::CloseHandle($h)
}

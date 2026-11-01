# One flatbed scan through WIA: colour, full bed, uncompressed BMP (no JPEG: it would blur the
# colour detail the pages test). Only scans the device whose name matches -Device.
param([string]$Out = "scan600.bmp", [int]$Dpi = 600, [string]$Device = "*")
$ErrorActionPreference = 'Stop'
$manager = New-Object -ComObject WIA.DeviceManager
$info = $manager.DeviceInfos | Where-Object { $_.Type -eq 1 -and $_.Properties.Item('Name').Value -like $Device } | Select-Object -First 1
if (-not $info) { throw "no scanner matching '$Device'" }
"device: " + $info.Properties.Item('Name').Value
$dev = $info.Connect()
foreach ($p in $dev.Properties) { if ($p.PropertyID -eq 3088 -and -not $p.IsReadOnly) { $p.Value = 2 } }  # flatbed
$item = $dev.Items.Item(1)
foreach ($p in $item.Properties) {
    switch ($p.PropertyID) {
        6146 { $p.Value = 1 }      # colour
        6147 { $p.Value = $Dpi }   # horizontal resolution
        6148 { $p.Value = $Dpi }   # vertical resolution
        6149 { $p.Value = 0 }
        6150 { $p.Value = 0 }
    }
}
foreach ($p in $item.Properties) { if ($p.PropertyID -in 6151, 6152) { $p.Value = $p.SubTypeMax } }  # whole bed
$image = $item.Transfer('{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}')  # BMP
"scanned " + $image.Width + "x" + $image.Height + " at " + [math]::Round($image.HorizontalResolution) + " dpi"
$path = [System.IO.Path]::GetFullPath($Out)
if (Test-Path $path) { Remove-Item $path }
$image.SaveFile($path)
"saved " + $path

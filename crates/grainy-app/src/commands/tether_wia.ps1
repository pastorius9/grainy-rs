$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$manager = New-Object -ComObject WIA.DeviceManager
$cameras = @($manager.DeviceInfos | Where-Object Type -eq 2)
if ($cameras.Count -ne 1) { throw 'Connect one WIA still camera with remote shutter support, or install gphoto2.' }
$device = $cameras[0].Connect()
$takePicture = '{AF933CAC-ACAD-11D2-A093-00C04F72DC3C}'
if (-not @($device.Commands | Where-Object CommandID -eq $takePicture).Count) {
    throw 'The WIA driver does not support remote capture.'
}
$item = $device.ExecuteCommand($takePicture)
if ($null -eq $item) { throw 'The camera did not return a captured item.' }
$image = $item.Transfer()
$extension = $image.FileExtension
if ($extension -notmatch '^[a-zA-Z0-9]{1,8}$') { throw 'Unknown camera file extension.' }
$image.SaveFile((Join-Path (Get-Location).Path "capture-1.$extension"))

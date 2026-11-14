# Copia ffmpeg.exe e ffprobe.exe instalados para a pasta de recursos do app,
# para que o instalador funcione em máquinas sem FFmpeg.
# Atenção à licença: builds "full" do FFmpeg são GPL; distribua o código-fonte
# correspondente ou use uma build LGPL (veja docs/SECURITY.md e o README).
$ErrorActionPreference = "Stop"
$dest = Join-Path $PSScriptRoot "..\apps\desktop\src-tauri\resources\ffmpeg"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
foreach ($tool in "ffmpeg", "ffprobe") {
  $src = (Get-Command $tool).Source
  Copy-Item $src $dest -Force
  Write-Host "copiado: $src"
}

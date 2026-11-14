# Instala dependências de desenvolvimento no Windows (winget).
$ErrorActionPreference = "Stop"

winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Microsoft.EdgeWebView2Runtime -e
winget install --id Gyan.FFmpeg -e

Write-Host ""
Write-Host "Reabra o terminal para atualizar o PATH e rode:"
Write-Host "  cd apps\desktop; npm install; npm run tauri dev"
Write-Host "Para empacotar o FFmpeg junto do instalador, use scripts\fetch-ffmpeg.ps1"

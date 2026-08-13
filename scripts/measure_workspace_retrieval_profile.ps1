$ErrorActionPreference = "Stop"

$visiblePaths = @(rg --files)
$textFileCount = 0
$textBytes = [int64]0
$textLines = [int64]0
$chunkCount = [int64]0

foreach ($relativePath in $visiblePaths) {
    $item = Get-Item -LiteralPath $relativePath -ErrorAction SilentlyContinue
    if (-not $item -or $item.Length -gt 1MB) {
        continue
    }

    $contentBytes = [System.IO.File]::ReadAllBytes($item.FullName)
    if ($contentBytes -contains 0) {
        continue
    }

    try {
        $lineCount = ([System.IO.File]::ReadLines($item.FullName) | Measure-Object).Count
    }
    catch {
        continue
    }

    $textFileCount++
    $textBytes += $item.Length
    $textLines += $lineCount
    if ($lineCount -gt 0) {
        $chunkCount += [math]::Ceiling($lineCount / 80)
    }
}

[pscustomobject]@{
    visibleFiles = $visiblePaths.Count
    textEnvelopeFiles = $textFileCount
    textBytes = $textBytes
    textLines = $textLines
    chunks80Lines = $chunkCount
    vectors384Bytes = $chunkCount * 384 * 4
    vectors768Bytes = $chunkCount * 768 * 4
} | ConvertTo-Json

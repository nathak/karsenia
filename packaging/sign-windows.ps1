# Signs tenrec-desktop.exe with a self-signed code-signing certificate (for your own PCs, or a company that trusts the certificate).
# Run it in PowerShell on the Windows PC that will run the program, next to the .exe:
#     powershell -ExecutionPolicy Bypass -File .\sign-windows.ps1
# ASCII only on purpose: Windows PowerShell 5.1 reads a file without a byte order mark as ANSI.
#
# - The certificate and its private key stay in this user's certificate store (Cert:\CurrentUser\My). The key cannot be exported: it is
#   never in a file, the repository, a zip or the NAS. A new PC (or a new Windows profile) makes its own certificate.
# - Only the public certificate is written (Tenrec-codesign.cer).
# - Signing changes the .exe: do not change it after (a rebuild, strip or a resource editor breaks the signature).
param(
    [string]$Path = (Join-Path $PSScriptRoot 'tenrec-desktop.exe'),
    [string]$Subject = 'CN=Tenrec',
    [string]$TimestampServer = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
$Path = (Resolve-Path -LiteralPath $Path).ProviderPath

# A certificate made here before: self-signed, with its private key, not about to expire.
$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
    Where-Object { $_.Subject -eq $Subject -and $_.Issuer -eq $_.Subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date).AddDays(30) } |
    Sort-Object NotAfter -Descending | Select-Object -First 1
if (-not $cert) {
    if (-not (Get-Command New-SelfSignedCertificate -ErrorAction SilentlyContinue) -or -not (Get-Command New-SelfSignedCertificate).Parameters.ContainsKey('KeyExportPolicy')) {
        throw 'This Windows has no New-SelfSignedCertificate with -KeyExportPolicy (Windows 10 or Server 2016 or newer is needed).'
    }
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject -FriendlyName 'Tenrec code signing' `
        -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 -KeyExportPolicy NonExportable `
        -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(3)
    Write-Host "Made a certificate: $($cert.Thumbprint)"
} else {
    Write-Host "Using the certificate made before: $($cert.Thumbprint)"
}
$cer = Join-Path (Split-Path -Parent $Path) 'Tenrec-codesign.cer'
Export-Certificate -Cert $cert -FilePath $cer -Type CERT | Out-Null

# SHA256 is named because Windows PowerShell 5.1 signs with SHA1 otherwise.
function Sign([string]$stamp) {
    $a = @{ FilePath = $Path; Certificate = $cert; HashAlgorithm = 'SHA256'; IncludeChain = 'All' }
    if ($stamp) { $a.TimestampServer = $stamp }
    try { $null = Set-AuthenticodeSignature @a } catch { Write-Warning $_.Exception.Message }
}
function Mine($s) { $s.SignerCertificate -and $s.SignerCertificate.Thumbprint -eq $cert.Thumbprint }

Sign $TimestampServer
$sig = Get-AuthenticodeSignature -FilePath $Path
if (-not (Mine $sig) -or -not $sig.TimeStamperCertificate) {
    Write-Warning 'No timestamp (offline or a proxy?): signing without one. That signature stops verifying when the certificate expires.'
    Sign $null
    $sig = Get-AuthenticodeSignature -FilePath $Path
}
# A self-signed certificate that is not trusted yet gives status UnknownError / NotTrusted: that is expected. A real failure is none of these.
if (-not (Mine $sig) -or $sig.Status -in 'NotSigned', 'HashMismatch', 'NotSupportedFileFormat', 'Incompatible') {
    throw "Signing failed: $($sig.Status) $($sig.StatusMessage) (is the program still running? close it and try again)"
}

Write-Host ''
Write-Host "Signed:      $Path"
Write-Host "Signer:      $($sig.SignerCertificate.Subject)  thumbprint $($sig.SignerCertificate.Thumbprint)  expires $($sig.SignerCertificate.NotAfter.ToString('yyyy-MM-dd'))"
Write-Host "Status:      $($sig.Status) (not 'Valid' until this PC trusts the certificate)"
Write-Host "Timestamp:   $(if ($sig.TimeStamperCertificate) { 'yes' } else { 'no' })"
Write-Host "SHA-256:     $((Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLower())   (changed by the signature; use it for an exception by hash)"
Write-Host "Certificate: $cer   (public part only)"
Write-Host ''
Write-Host 'To be trusted, a PC needs that .cer in "Trusted Root Certification Authorities" and "Trusted Publishers" (a company: its administrator,'
Write-Host 'by group policy). A security product keeps its own rules: its administrator must allow this certificate, folder or file hash.'

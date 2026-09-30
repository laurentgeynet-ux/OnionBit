param([string]$Path)
$errs = $null
[void][System.Management.Automation.PSParser]::Tokenize((Get-Content $Path -Raw), [ref]$errs)
if ($errs -and $errs.Count -gt 0) { $errs | ForEach-Object { $_.Message }; exit 1 }
'syntaxe OK'

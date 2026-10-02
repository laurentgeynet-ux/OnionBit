# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# OnionBit -- echantillonneur de statistiques reseau pour l'analyse de
# fingerprinting (threat model : "Fingerprinting de l'implementation").
#
# Interroge periodiquement les compteurs REST `GET /api/ipv8/overlays/
# statistics` (num_up/num_down/bytes_up/bytes_down par overlay et par
# msg_id) et `GET /api/ipv8/tunnel/circuits` (nombre de circuits par
# etat). Produit un CSV agrege -- pas de capture de paquets : on
# conserve des statistiques, jamais un PCAP brut.
#
# Le meme script fonctionne contre un Tribler officiel (meme shape de
# reponse) : lancer une session idle 10-30 min puis des transferts,
# des deux cotes, puis comparer les deltas (debits par msg_id,
# rafales, cadences keepalive).
#
# Usage :
#   .\scripts\fingerprint_stats.ps1 -ApiBase http://127.0.0.1:52100 `
#       -ApiKey <cle> -DurationMin 20 -IntervalSec 5 `
#       -OutCsv fingerprint_onionbit.csv
#
# NOTE encodage : fichier volontairement ASCII -- un caractere
# multi-octets lu en CP1252 par Windows PowerShell 5.1 peut casser le
# parsing (cf. fuzz_campaign.ps1).
param(
    [string]$ApiBase = "http://127.0.0.1:52100",
    [string]$ApiKey = "",
    [int]$DurationMin = 20,
    [int]$IntervalSec = 5,
    [string]$OutCsv = ""
)

$ErrorActionPreference = "Stop"
if ($OutCsv -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutCsv = "fingerprint_$stamp.csv"
}

$headers = @{}
if ($ApiKey -ne "") { $headers["X-Api-Key"] = $ApiKey }

"ts,overlay,msg,num_up,num_down,bytes_up,bytes_down,circuits_ready,circuits_total" |
    Out-File $OutCsv -Encoding utf8

$deadline = (Get-Date).AddMinutes($DurationMin)
$samples = 0
while ((Get-Date) -lt $deadline) {
    $t0 = Get-Date
    $ts = $t0.ToString("yyyy-MM-ddTHH:mm:ss.fff")

    $ready = 0; $total = 0
    try {
        $circ = Invoke-RestMethod -Uri "$ApiBase/api/ipv8/tunnel/circuits" `
            -Headers $headers -TimeoutSec $IntervalSec
        if ($circ.circuits) {
            $total = @($circ.circuits).Count
            $ready = @($circ.circuits | Where-Object { $_.state -eq "READY" }).Count
        }
    } catch {
        # Daemon sans stack ipv8 ou indisponible : on journalise
        # quand meme les compteurs d'overlay (0 si absents).
    }

    try {
        $stats = Invoke-RestMethod -Uri "$ApiBase/api/ipv8/overlays/statistics" `
            -Headers $headers -TimeoutSec $IntervalSec
        foreach ($entry in @($stats.statistics)) {
            foreach ($prop in $entry.PSObject.Properties) {
                $overlay = $prop.Name
                foreach ($m in $prop.Value.PSObject.Properties) {
                    $s = $m.Value
                    "$ts,$overlay,$($m.Name),$($s.num_up),$($s.num_down),$($s.bytes_up),$($s.bytes_down),$ready,$total" |
                        Out-File $OutCsv -Append -Encoding utf8
                }
            }
        }
        $samples++
    } catch {
        Write-Host ("{0} : echantillon ignore ({1})" -f $ts, $_.Exception.Message)
    }

    $elapsed = ((Get-Date) - $t0).TotalSeconds
    $wait = $IntervalSec - $elapsed
    if ($wait -gt 0) { Start-Sleep -Seconds $wait }
}

Write-Host ("fingerprint_stats : {0} echantillons -> {1}" -f $samples, $OutCsv)

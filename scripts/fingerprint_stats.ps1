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
    [string]$OutCsv = "",
    # ADR-0015 : si fourni, chaque tick echantillonne aussi
    # GET /api/ipv8/ext (compteurs hello/attest) vers ce CSV.
    [string]$ExtCsv = ""
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

if ($ExtCsv -ne "") {
    "ts,peer_count,hello_tx,hello_probed,attest_rx,attest_dropped,attest_stored,attest_tx,ledger_rx,ledger_dropped,ledger_stored,ledger_tx,ledger_links,ledger_pending,ledger_forks" |
        Out-File $ExtCsv -Encoding utf8
}

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

    # Compteurs agreges communs aux deux implementations (l'endpoint
    # Rust de Tribler ne publie pas les stats par overlay ci-dessus) :
    # octets totaux de l'endpoint, sommes par objet de routage
    # (circuits/relays/exits) et totaux libtorrent + taille DB.
    try {
        $ep = Invoke-RestMethod -Uri "$ApiBase/api/statistics/ipv8" `
            -Headers $headers -TimeoutSec $IntervalSec
        $eu = $ep.ipv8_statistics.total_up; $ed = $ep.ipv8_statistics.total_down
        "$ts,(endpoint),total,0,0,$eu,$ed,$ready,$total" |
            Out-File $OutCsv -Append -Encoding utf8
    } catch {}
    foreach ($pair in @(@('circuits','circuits'), @('relays','relays'), @('exits','exits'))) {
        try {
            $r = Invoke-RestMethod -Uri "$ApiBase/api/ipv8/tunnel/$($pair[0])" `
                -Headers $headers -TimeoutSec $IntervalSec
            $items = @($r.($pair[1]))
            $bu = 0; $bd = 0
            foreach ($i in $items) { $bu += $i.bytes_up; $bd += $i.bytes_down }
            "$ts,(tunnel),$($pair[0]),$($items.Count),$($items.Count),$bu,$bd,$ready,$total" |
                Out-File $OutCsv -Append -Encoding utf8
        } catch {}
    }
    try {
        $ts2 = Invoke-RestMethod -Uri "$ApiBase/api/statistics/tribler" `
            -Headers $headers -TimeoutSec $IntervalSec
        $lb = $ts2.tribler_statistics.libtorrent
        "$ts,(libtorrent),total,0,0,$($lb.total_sent_bytes),$($lb.total_recv_bytes),$ready,$total" |
            Out-File $OutCsv -Append -Encoding utf8
        "$ts,(db),size,0,0,$($ts2.tribler_statistics.db_size),0,$ready,$total" |
            Out-File $OutCsv -Append -Encoding utf8
    } catch {}

    # Compteurs ext (ADR-0015) — memes lignes quelle que soit la
    # cible ; quand ext est desactive l'endpoint repond `enabled:false`
    # avec des compteurs a zero.
    if ($ExtCsv -ne "") {
        try {
            $x = Invoke-RestMethod -Uri "$ApiBase/api/ipv8/ext" `
                -Headers $headers -TimeoutSec $IntervalSec
            $e = $x.ext
            $pc = if ($null -ne $e.peer_count) { $e.peer_count } else { 0 }
            $ht = if ($null -ne $e.hello_tx) { $e.hello_tx } else { 0 }
            $hp = if ($null -ne $e.hello_probed) { $e.hello_probed } else { 0 }
            $ar = if ($null -ne $e.attest_rx) { $e.attest_rx } else { 0 }
            $ad = if ($null -ne $e.attest_dropped) { $e.attest_dropped } else { 0 }
            $as = if ($null -ne $e.attest_stored) { $e.attest_stored } else { 0 }
            $at = if ($null -ne $e.attest_tx) { $e.attest_tx } else { 0 }
            # Compteurs ledger (Phase 9c) — endpoint separe
            # `ext/ledger` ; zero constant = pas de tranche en vol.
            $lr = 0; $ld = 0; $ls = 0; $lt = 0; $ll = 0; $lp = 0; $lf = 0
            try {
                $lg = Invoke-RestMethod -Uri "$ApiBase/api/ipv8/ext/ledger" `
                    -Headers $headers -TimeoutSec $IntervalSec
                $l = $lg.ledger
                if ($null -ne $l.rx)          { $lr = $l.rx }
                if ($null -ne $l.dropped)     { $ld = $l.dropped }
                if ($null -ne $l.stored)      { $ls = $l.stored }
                if ($null -ne $l.tx)          { $lt = $l.tx }
                if ($null -ne $l.links_count) { $ll = $l.links_count }
                if ($null -ne $l.pending)     { $lp = $l.pending }
                if ($null -ne $l.forks)       { $lf = $l.forks }
            } catch {}
            "$ts,$pc,$ht,$hp,$ar,$ad,$as,$at,$lr,$ld,$ls,$lt,$ll,$lp,$lf" |
                Out-File $ExtCsv -Append -Encoding utf8
        } catch {}
    }

    $elapsed = ((Get-Date) - $t0).TotalSeconds
    $wait = $IntervalSec - $elapsed
    if ($wait -gt 0) { Start-Sleep -Seconds $wait }
}

Write-Host ("fingerprint_stats : {0} echantillons -> {1}" -f $samples, $OutCsv)

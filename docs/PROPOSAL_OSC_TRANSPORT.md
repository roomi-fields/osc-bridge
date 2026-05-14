# Proposition — Transport OSC en sortie (drivers logiciels)

**Statut :** ✅ toutes décisions actées le 2026-05-13. Prêt pour l'implémentation (plan d'adoption étape 1 : runtime + premier driver pilote AbletonOSC).
**Auteur :** roomi-fields
**Date :** 2026-05-13

## Contexte et motivation

Le bridge sait aujourd'hui traduire OSC entrant en MIDI/SysEx sortant vers un appareil hardware. La grande majorité des cibles intéressantes côté logiciel — Ableton via AbletonOSC, Sonic Pi, SuperCollider, Pure Data, Reaper, Bitwig — exposent déjà une surface OSC native. Étendre le bridge pour qu'il émette aussi de l'OSC en sortie permet d'unifier sous un même catalogue de drivers : synthés hardware *et* environnements logiciels. Du point de vue d'un client (ou d'un LLM via le futur MCP), il n'y a plus qu'une surface OSC nommée à apprendre, quelle que soit la nature de la cible.

## Extension du schéma

Une seule notion nouvelle au niveau `device` :

```jsonc
{
  "device": {
    "name": "Ableton Live",
    "vendor": "Ableton",
    "kind": "software",
    "osc_prefix": "/ableton",
    "transport": {
      "kind": "osc",
      "host": "127.0.0.1",
      "port": 11000
    }
  }
}
```

`device.kind` vaut `"hardware"` par défaut (back-compat totale, les 840 drivers existants ne bougent pas). `transport.kind` vaut `"midi"` par défaut.

Les commandes utilisent un champ `forward` à la place de `frame` :

```jsonc
{
  "commands": [
    {
      "osc": "/transport/play",
      "forward": { "path": "/live/song/start_playing", "args": [] }
    },
    {
      "osc": "/transport/tempo",
      "args": [{"name": "bpm", "type": "float"}],
      "forward": { "path": "/live/song/set/tempo", "args": ["{bpm}"] }
    },
    {
      "osc": "/track/{n}/volume",
      "args": [
        {"name": "n", "type": "u8"},
        {"name": "v", "type": "float", "range": [0.0, 1.0]}
      ],
      "forward": { "path": "/live/track/set/volume", "args": ["{n}", "{v}"] }
    }
  ]
}
```

Les types OSC (`int`, `float`, `string`) sont déclarés explicitement côté args et forward — pas d'auto-détection. Cela évite les confusions entre `120` (int) et `120.0` (float), qui rendent AbletonOSC silencieux.

## Souscriptions et replies

Les environnements OSC (AbletonOSC, SC) acceptent qu'on s'abonne à un état distant. Deux primitives :

```jsonc
{
  "subscriptions": [
    {
      "on": "startup",
      "forward": { "path": "/live/song/start_listen/tempo", "args": [] }
    }
  ],
  "replies": [
    {
      "match_osc": "/live/song/get/tempo",
      "match_args": [{"name": "bpm", "type": "float"}],
      "emit_osc": "/transport/tempo {bpm}"
    },
    {
      "match_osc": "/live/track/get/volume",
      "match_args": [
        {"name": "n", "type": "int"},
        {"name": "v", "type": "float"}
      ],
      "emit_osc": "/track/{n}/volume {v}"
    }
  ]
}
```

À l'import du driver, le runtime émet les souscriptions une fois. Les messages OSC entrants depuis la cible sont matchés contre `replies[]` et re-émis vers les clients OSC du bridge avec le préfixe canonique du driver.

## Exemple complet — `devices/ableton/live.third-party-osc.fw-12.1.json`

```jsonc
{
  "_sources": [{
    "tier": "📡 third-party-osc",
    "name": "AbletonOSC by ideoforms",
    "url": "https://github.com/ideoforms/AbletonOSC",
    "version": "0.4",
    "firmware": "Ableton Live 12.1"
  }],
  "_coverage": { "host_software": "Ableton Live 12.1", "shim_version": "AbletonOSC 0.4" },
  "device": {
    "name": "Ableton Live",
    "vendor": "Ableton",
    "kind": "software",
    "revision": "Ableton Live 12.1 + AbletonOSC 0.4",
    "osc_prefix": "/ableton",
    "transport": { "kind": "osc", "host": "127.0.0.1", "port": 11000 }
  },
  "commands": [
    { "osc": "/transport/play",
      "forward": { "path": "/live/song/start_playing", "args": [] } },
    { "osc": "/transport/stop",
      "forward": { "path": "/live/song/stop_playing", "args": [] } },
    { "osc": "/transport/tempo",
      "args": [{"name": "bpm", "type": "float", "range": [20.0, 999.0]}],
      "forward": { "path": "/live/song/set/tempo", "args": ["{bpm}"] } },
    { "osc": "/track/{n}/volume",
      "args": [{"name": "n","type":"u8"},{"name":"v","type":"float","range":[0.0,1.0]}],
      "forward": { "path": "/live/track/set/volume", "args": ["{n}", "{v}"] } },
    { "osc": "/track/{n}/arm",
      "args": [{"name":"n","type":"u8"},{"name":"on","type":"bool"}],
      "forward": { "path": "/live/track/set/arm", "args": ["{n}", "{on}"] } },
    { "osc": "/scene/{n}/fire",
      "args": [{"name":"n","type":"u8"}],
      "forward": { "path": "/live/scene/fire", "args": ["{n}"] } }
  ],
  "subscriptions": [
    { "on": "startup",
      "forward": { "path": "/live/song/start_listen/tempo", "args": [] } }
  ],
  "replies": [
    { "match_osc": "/live/song/get/tempo",
      "match_args": [{"name":"bpm","type":"float"}],
      "emit_osc": "/transport/tempo {bpm}" }
  ]
}
```

Surface vue par le client OSC du bridge :

```
/ableton/transport/play
/ableton/transport/stop
/ableton/transport/tempo 124.5
/ableton/track/0/volume 0.8
/ableton/scene/3/fire
```

Et en entrée vers le client, automatique dès qu'Ableton change de tempo :

```
/ableton/transport/tempo 126.0
```

## Découverte des cibles

Pas d'auto-énumération possible : les cibles OSC ne s'annoncent pas. Deux options non exclusives :

1. **Inline dans le driver JSON** (`transport.host` + `transport.port`) — bon pour les setups fixes.
2. **Override via `orchestrate.toml`** — pour pointer plusieurs instances du même environnement (deux Sonic Pi sur le LAN), surcharger l'IP par défaut, ou tester localement vs setup studio.

Le bridge tente la résolution dans cet ordre : flag CLI > `orchestrate.toml` > driver JSON.

## Décisions

1. **Hiérarchie de fichiers.** ✅ **Catalogue unifié** : `devices/<vendor>/<software>.json` avec un champ `kind: "software"` au niveau `device` pour filtrage. Pas de répertoire `targets/` séparé.

2. **Tiers de source pour drivers logiciels.** ✅ **Ajout de deux tiers** : `📡 vendor-osc-api` (Reaper, Bitwig DrivenByMoss officiel) et `📡 third-party-osc` (AbletonOSC et autres remote-scripts communautaires). Même logique de traçabilité que les tiers hardware.

3. **Pinning de version — multi-version traité comme un must-have.** ✅ **Le nom de fichier pin la version du logiciel hôte** (`live.third-party-osc.fw-12.1.json` pour Ableton 12.1) ; la version du shim OSC tiers (AbletonOSC 0.4) va dans `_sources[].version`, et `revision` capture la combinaison testée (`"Ableton Live 12.1 + AbletonOSC 0.4"`). Plusieurs variantes d'un même logiciel coexistent dans le catalogue, exactement comme pour le hardware. Le catalogue regroupe automatiquement les variantes sous une entrée logique unique (mécanique déjà en place dans `regen_supported_devices.py`).

   Exemple d'arborescence cible pour Ableton :

   ```
   devices/ableton/
     live.third-party-osc.fw-11.json
     live.third-party-osc.fw-12.0.json
     live.third-party-osc.fw-12.1.json
   ```

4. **Types OSC supportés.** ✅ **Démarrage minimal** : `int`, `float`, `string`, `bool`. Extension à `blob`, `int64`, `timetag` faite à la demande, quand un driver concret le requiert. Le moteur loggue et rejette poliment un type non supporté pendant l'import du driver.

5. **Lua scripting.** ✅ **Étendu aux drivers OSC.** Les blocs `transform` et `script` valent pour tous les transports. Mêmes garde-fous (sandbox, 1 MiB, 10 ms), même API `ob.*`. Utile notamment pour Reaper (`f32` à clamper sur des courbes spécifiques) ou pour des shims qui exigent un re-formatage.

6. **Réémission bidirectionnelle / événements push.** ✅ **Réutilisation du mécanisme `replies[]` actuel sans extension**. Un event AbletonOSC du type `/live/clip/started [track, clip]` se matche exactement comme une réponse sollicitée. Si un cas réel demande filtrage, état, ou conditionnel, on tombe dans l'escape hatch Lua. On enrichit le schéma seulement si un driver pilote en a besoin.

7. **Rate-limiting sortant en OSC.** ✅ **Pas de throttle par défaut**, le champ `rate_limit_hz` reste optionnel et applicable au transport OSC pour les rares cibles qui décrochent (boucles Python lentes côté receveur). Le contributeur le déclare uniquement s'il a observé des drops.

## Plan d'adoption

Trois étapes, séquentielles :

1. **Schéma + runtime minimal.** Implémenter `transport.kind = "osc"`, `forward`, `subscriptions`, `replies` (OSC entrant). Aucun changement aux drivers MIDI existants. Tests unitaires sur un driver factice + un driver AbletonOSC réel.
2. **Drivers pilotes.** Trois cibles d'amorçage qui couvrent les trois familles : AbletonOSC (DAW), Sonic Pi (live coding), SuperCollider (synthèse). Toutes les questions ouvertes ci-dessus se résoudront pendant ces trois drivers.
3. **Catalogue logiciel.** Une fois les trois pilotes validés, ouvrir aux contributions et porter les autres cibles identifiées (cf. ci-dessous).

## Cibles candidates pour l'amorçage du catalogue

| Cible | Source OSC | Tier | Notes |
|---|---|---|---|
| Ableton Live | [AbletonOSC](https://github.com/ideoforms/AbletonOSC) | `third-party-osc` | Remote Script tiers, OSC pur, mainstream. |
| Sonic Pi | OSC natif | `vendor-osc-api` | `live_loop` + `sync :osc/...` côté Sonic Pi. |
| SuperCollider | `NetAddr` / `OSCdef` | `vendor-osc-api` | Surface définie côté SC par l'utilisateur — driver générique + helpers. |
| Pure Data | `netreceive` | `vendor-osc-api` | Idem SC. |
| Reaper | OSC natif (Control Surfaces) | `vendor-osc-api` | Fichier de mapping `.ReaperOSC` côté Reaper. |
| Bitwig | [DrivenByMoss](https://github.com/git-moss/DrivenByMoss) (OSC mode) | `third-party-osc` | Officiellement supporté par Bitwig en partenariat. |
| TouchDesigner | OSC natif | `vendor-osc-api` | Pour les workflows AV. |
| VCV Rack | [Stoermelder MINDMELD VCV-OSC](https://github.com/stoermelder/vcvrack-packone) | `third-party-osc` | Synthé modulaire logiciel. |

DAW hors couverture native (pas d'OSC officiel) : Logic Pro, FL Studio, Pro Tools, Cubase. Restent du ressort des MCPs spécialisés tiers.

## Ce que la proposition ne change pas

- Aucun driver MIDI existant n'a besoin d'être modifié.
- L'API OSC vue par les clients reste la même : chemin nommé sous `osc_prefix`, args typés.
- Le runtime Rust gagne une branche de dispatch ; pas de refonte.
- Le futur MCP voit la même surface (`list_devices`, `send`, `get_routes`, `list_subscriptions`), avec en plus quelques champs informatifs (`kind: hardware|software`, `transport: midi|osc`).

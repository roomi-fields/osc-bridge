# Proposition — Transport WebSocket (clients navigateur)

**Statut :** 🕐 en attente d'arbitrage.
**Auteur :** roomi-fields (rédaction Claude)
**Date :** 2026-08-20

## Contexte et motivation

Un navigateur ne parle pas UDP : aucune page web ne peut aujourd'hui être
client OSC du bridge. Premier cas d'usage concret : l'instrument binaural
(`~/dev/music/binaural`), une appli Web Audio pilotée par un MiniLab 3 — le
bridge décode le matériel et renvoie l'affichage OLED / les couleurs de pads.
La fonctionnalité est générique : n'importe quel client web (interfaces de
contrôle, visualiseurs, TouchDesigner-web…) en profite.

## Décisions à acter

1. **CLI** : un drapeau `--ws-port <PORT>` sur `run` (étape 1) puis
   `orchestrate` (étape 2). Désactivé par défaut. Écoute sur `127.0.0.1`
   (un `--ws-bind` optionnel pour exposer plus large, à ses risques).
2. **Format de trame** : trames WebSocket **binaires = paquets OSC bruts**,
   exactement les mêmes octets que l'UDP (encodage rosc). Pas de JSON : un
   seul encodeur des deux côtés, symétrie totale avec le fan-out existant.
   Côté navigateur, un encodeur/décodeur OSC minimal (~100 lignes de JS sans
   dépendance) suffit.
3. **Sémantique client** : un client WS connecté ≡ un `--osc-client` — il
   reçoit tout le flux sortant (MIDI-in décodé, réponses SysEx,
   `/bridge/status`) ; ses trames entrantes passent par le **même dispatch**
   que l'OSC UDP entrant. Aucune nouvelle surface : mêmes adresses, mêmes
   arguments.
4. **Implémentation** : `tungstenite` en mode synchrone — un fil d'exécution
   d'acceptation + un fil par connexion, conforme à l'architecture actuelle
   (fils + `UdpSocket`, pas de runtime async à introduire).

## Mécanique (esquisse)

- Le broadcast sortant (`send_osc`) gagne une liste partagée d'émetteurs WS
  à côté de la liste UDP existante ; déconnexion = retrait silencieux.
- Le serveur WS démarre après la construction des tables de dispatch et
  partage les mêmes `Arc` que la boucle UDP ; chaque trame binaire décodée
  est passée à `dispatch(...)` à l'identique.
- Rate-limiting et backpressure existants inchangés (les messages WS entrent
  dans le même chemin que l'UDP).

## Plan d'adoption

Étape 1 : `run` + validation de bout en bout avec l'instrument binaural
(MiniLab 3 réel). Étape 2 : `orchestrate`. Étape 3 : documentation README +
mention sur la page catalogue.

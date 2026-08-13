# Tuwaiq AI System Assistant + AI System Intelligence (Prototype)

This directory contains an independent prototype for future AI-native capabilities in TuwaiqOS.
It is isolated from the current TuwaiqOS runtime and does not modify kernel code.

## Purpose

Two conceptual components are defined:

1. Tuwaiq AI Assistant (future)
- Maps user questions into structured system-information requests.
- Enforces policy and permission boundaries before any system access.

2. Tuwaiq AI System Intelligence (implemented prototype focus)
- Collects telemetry (synthetic for now).
- Validates and processes telemetry.
- Trains an anomaly-detection baseline (Isolation Forest).
- Exports model artifacts and metadata.
- Supports standalone inference and evaluation.

## Current Implementation Status

Implemented now:
- Synthetic telemetry generation (deterministic, seed-based).
- Telemetry schema and validation.
- Standalone collector abstraction.
- Training pipeline using Isolation Forest.
- Exported model + metadata.
- Standalone inference engine with structured output.
- Evaluation pipeline and report generation.
- Automated tests for schema, preprocessing, inference, model reload, and interface readiness.

Planned/future:
- Real telemetry integration from TuwaiqOS APIs.
- Assistant-to-intelligence orchestration with LLM tool-calling.
- Policy engine integration with runtime permissions.

## Directory Structure

- data_collection: telemetry schema, validation, collector abstraction
- data: synthetic/raw/processed datasets
- training: configs, scripts, experiments
- models: checkpoints, exported artifacts, metadata
- inference: model inference engine, examples, tests
- evaluation: reports and benchmark outputs
- system_interface: conceptual integration schemas and API specs
- docs: architecture, dataset, training, model, integration docs

## Setup

From the ai_development directory:

```powershell
python -m venv .venv
.\.venv\Scripts\activate
pip install -r requirements.txt
```

## Generate Synthetic Data

```powershell
python training\scripts\generate_synthetic_data.py --rows 2400 --seed 42 --normal-ratio 0.85 --sampling-interval 5
```

Output:
- data/raw/telemetry_synthetic_v1.jsonl
- data/processed/telemetry_synthetic_v1.csv

Important:
- Data is synthetic and intended for development only.
- It is not real TuwaiqOS telemetry.

## Train

```powershell
python training\scripts\train.py
```

Outputs:
- models/exported/*.pkl
- models/metadata/*.json
- training/experiments/*.json

## Evaluate

```powershell
python training\scripts\evaluate.py
```

Outputs:
- evaluation/reports/*.md
- evaluation/benchmarks/*.json

Note:
- Evaluation metrics are based on synthetic labeled scenarios.

## Run Inference

Single input JSON file:

```powershell
python inference\predict.py --input-json inference\examples\normal_input.json
```

Example batch scenarios:

```powershell
python inference\examples\run_examples.py
```

## Run Tests

```powershell
pytest
```

## Integration Direction

This prototype does not integrate with kernel code yet.
See docs/INTEGRATION.md and system_interface/api_specs/future_integration_spec.md.

## Security and Safety Principles

- Read-only intelligence behavior in current prototype.
- No automatic destructive actions.
- No automatic process termination.
- No direct model-to-kernel communication.
- No telemetry network transmission unless explicitly configured in future work.
- No intentional collection of personal user data.

## Assistant Security Boundary (future)

User -> AI Assistant -> Context Manager -> AI Policy Layer -> Tuwaiq System API -> System Information

LLM -> Structured request/tool call -> Permission validation -> System API -> Kernel

The LLM must not receive direct kernel access.

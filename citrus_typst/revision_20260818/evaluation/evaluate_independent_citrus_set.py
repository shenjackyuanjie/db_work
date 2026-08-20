r"""Evaluate the deployed ONNX models on a traceable independent test set.

The CSV manifest must follow citrus_typst/independent_test_manifest_template.csv.
This script deliberately rejects rows that have not been marked as permission-confirmed
and training-overlap-checked. It is not intended for the archived training data.

Example:
  ..\navel_back\.venv\Scripts\python.exe citrus_typst\revision_20260818\evaluation\evaluate_independent_citrus_set.py ^
    --manifest citrus_typst\revision_20260818\evaluation\independent_test_manifest.csv ^
    --output-dir citrus_typst\revision_20260818\evaluation_output
"""

from __future__ import annotations

import argparse
import csv
import json
import math
from collections import Counter
from pathlib import Path
from typing import Any

import numpy as np
import onnxruntime as ort
from PIL import Image


GATE_CLASSES = ["非果树", "是果树"]
DISEASE_CLASSES = ["黄龙病", "健康果树", "溃疡病", "沙皮病"]
MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)
THRESHOLDS = (0.50, 0.60, 0.70, 0.75, 0.80, 0.90)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Evaluate the deployed citrus ONNX models on an independent manifest."
    )
    parser.add_argument("--manifest", type=Path, required=True, help="CSV test manifest")
    parser.add_argument(
        "--data-root",
        type=Path,
        default=None,
        help="Base directory for relative file_path entries (defaults to manifest parent)",
    )
    parser.add_argument(
        "--model-1",
        type=Path,
        default=Path("onnx/model_1.onnx"),
        help="Gate-model ONNX path",
    )
    parser.add_argument(
        "--model-2",
        type=Path,
        default=Path("onnx/model_2.onnx"),
        help="Disease-model ONNX path",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="Directory for JSON, CSV, and confusion-matrix outputs",
    )
    return parser.parse_args()


def softmax(logits: np.ndarray) -> np.ndarray:
    shifted = logits - np.max(logits)
    exp = np.exp(shifted)
    return exp / np.sum(exp)


def preprocess(image_path: Path) -> np.ndarray:
    """Match the Rust service: RGB, 224x224, triangle/bilinear resize, ImageNet norm."""
    with Image.open(image_path) as image:
        rgb = image.convert("RGB").resize((224, 224), Image.Resampling.BILINEAR)
        array = np.asarray(rgb, dtype=np.float32) / 255.0
    normalized = (array - MEAN) / STD
    return np.transpose(normalized, (2, 0, 1))[np.newaxis, ...].astype(np.float32)


def load_manifest(manifest_path: Path, data_root: Path) -> list[dict[str, str]]:
    required = {
        "sample_id",
        "file_path",
        "task",
        "true_label",
        "source",
        "acquired_date",
        "permission_status",
        "annotator",
        "reviewer",
        "training_overlap_checked",
    }
    with manifest_path.open("r", encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None:
            raise ValueError("Manifest is missing a header row.")
        missing = required.difference(reader.fieldnames)
        if missing:
            raise ValueError(f"Manifest is missing columns: {', '.join(sorted(missing))}")
        rows = list(reader)

    if not rows:
        raise ValueError("Manifest contains no samples.")

    accepted: list[dict[str, str]] = []
    for index, row in enumerate(rows, start=2):
        sample_id = row["sample_id"].strip()
        if not sample_id or sample_id.startswith("EXAMPLE-"):
            raise ValueError(f"Row {index} contains a placeholder sample_id.")
        task = row["task"].strip().lower()
        if task not in {"gate", "disease"}:
            raise ValueError(f"Row {index} has unsupported task '{row['task']}'.")
        classes = GATE_CLASSES if task == "gate" else DISEASE_CLASSES
        true_label = row["true_label"].strip()
        if true_label not in classes:
            raise ValueError(
                f"Row {index} has label '{true_label}' incompatible with task '{task}'."
            )
        if row["permission_status"].strip().lower() != "confirmed":
            raise ValueError(f"Row {index} does not have permission_status=confirmed.")
        if row["training_overlap_checked"].strip().lower() != "yes":
            raise ValueError(f"Row {index} does not have training_overlap_checked=yes.")
        if not row["source"].strip() or not row["annotator"].strip() or not row["reviewer"].strip():
            raise ValueError(f"Row {index} is missing source, annotator, or reviewer metadata.")

        image_path = Path(row["file_path"].strip())
        if not image_path.is_absolute():
            image_path = data_root / image_path
        if not image_path.is_file():
            raise FileNotFoundError(f"Row {index} image does not exist: {image_path}")
        row["task"] = task
        row["true_label"] = true_label
        row["resolved_path"] = str(image_path.resolve())
        accepted.append(row)
    return accepted


def run_prediction(session: ort.InferenceSession, image_path: Path, classes: list[str]) -> tuple[str, float, list[float]]:
    input_name = session.get_inputs()[0].name
    logits = np.asarray(session.run(None, {input_name: preprocess(image_path)})[0]).reshape(-1)
    probabilities = softmax(logits)
    predicted_index = int(np.argmax(probabilities))
    if len(probabilities) != len(classes):
        raise RuntimeError(
            f"Model output has {len(probabilities)} classes; expected {len(classes)} for {classes}."
        )
    return classes[predicted_index], float(probabilities[predicted_index]), probabilities.tolist()


def classification_metrics(classes: list[str], rows: list[dict[str, Any]]) -> dict[str, Any]:
    matrix = [[0 for _ in classes] for _ in classes]
    index = {label: position for position, label in enumerate(classes)}
    for row in rows:
        matrix[index[row["true_label"]]][index[row["predicted_label"]]] += 1

    total = sum(sum(line) for line in matrix)
    correct = sum(matrix[i][i] for i in range(len(classes)))
    per_class: dict[str, dict[str, float | int]] = {}
    f1_values: list[float] = []
    for position, label in enumerate(classes):
        tp = matrix[position][position]
        fp = sum(matrix[row][position] for row in range(len(classes))) - tp
        fn = sum(matrix[position]) - tp
        precision = tp / (tp + fp) if tp + fp else 0.0
        recall = tp / (tp + fn) if tp + fn else 0.0
        f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
        per_class[label] = {
            "support": sum(matrix[position]),
            "precision": precision,
            "recall": recall,
            "f1": f1,
        }
        f1_values.append(f1)
    return {
        "sample_count": total,
        "accuracy": correct / total if total else math.nan,
        "macro_f1": sum(f1_values) / len(f1_values) if f1_values else math.nan,
        "classes": classes,
        "confusion_matrix": matrix,
        "per_class": per_class,
    }


def task_threshold_metrics(disease_rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    for threshold in THRESHOLDS:
        automatic = [
            row
            for row in disease_rows
            if row["predicted_label"] != "健康果树" and row["confidence"] >= threshold
        ]
        review = [row for row in disease_rows if row not in automatic]
        correct_automatic = sum(
            row["predicted_label"] == row["true_label"] and row["true_label"] != "健康果树"
            for row in automatic
        )
        review_errors = sum(row["predicted_label"] != row["true_label"] for row in review)
        results.append(
            {
                "threshold": threshold,
                "automatic_task_count": len(automatic),
                "automatic_task_ratio": len(automatic) / len(disease_rows) if disease_rows else math.nan,
                "automatic_task_correct_count": correct_automatic,
                "automatic_task_precision": correct_automatic / len(automatic) if automatic else math.nan,
                "manual_review_count": len(review),
                "manual_review_ratio": len(review) / len(disease_rows) if disease_rows else math.nan,
                "manual_review_error_ratio": review_errors / len(review) if review else math.nan,
            }
        )
    return results


def write_confusion_csv(path: Path, metrics: dict[str, Any]) -> None:
    with path.open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["true_label \\ predicted_label", *metrics["classes"]])
        for label, values in zip(metrics["classes"], metrics["confusion_matrix"]):
            writer.writerow([label, *values])


def main() -> None:
    args = parse_args()
    manifest_path = args.manifest.resolve()
    data_root = (args.data_root or manifest_path.parent).resolve()
    rows = load_manifest(manifest_path, data_root)
    for model_path in (args.model_1, args.model_2):
        if not model_path.is_file():
            raise FileNotFoundError(f"Model file does not exist: {model_path}")

    sessions = {
        "gate": ort.InferenceSession(str(args.model_1), providers=["CPUExecutionProvider"]),
        "disease": ort.InferenceSession(str(args.model_2), providers=["CPUExecutionProvider"]),
    }
    classes_by_task = {"gate": GATE_CLASSES, "disease": DISEASE_CLASSES}
    predictions: list[dict[str, Any]] = []
    for row in rows:
        task = row["task"]
        predicted_label, confidence, probabilities = run_prediction(
            sessions[task], Path(row["resolved_path"]), classes_by_task[task]
        )
        predictions.append(
            {
                **row,
                "predicted_label": predicted_label,
                "confidence": confidence,
                "probabilities": probabilities,
                "correct": predicted_label == row["true_label"],
            }
        )

    by_task = {task: [row for row in predictions if row["task"] == task] for task in classes_by_task}
    metrics = {
        task: classification_metrics(classes_by_task[task], by_task[task])
        for task in classes_by_task
        if by_task[task]
    }
    report = {
        "manifest": str(manifest_path),
        "data_root": str(data_root),
        "model_1": str(args.model_1.resolve()),
        "model_2": str(args.model_2.resolve()),
        "sample_counts": dict(Counter(row["task"] for row in predictions)),
        "metrics": metrics,
        "threshold_analysis": task_threshold_metrics(by_task["disease"]),
    }

    args.output_dir.mkdir(parents=True, exist_ok=True)
    with (args.output_dir / "evaluation_report.json").open("w", encoding="utf-8") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2, allow_nan=False)
    with (args.output_dir / "per_sample_predictions.csv").open("w", encoding="utf-8-sig", newline="") as handle:
        fieldnames = [
            "sample_id", "task", "true_label", "predicted_label", "confidence", "correct",
            "source", "acquired_date", "permission_status", "annotator", "reviewer",
            "training_overlap_checked", "resolved_path", "probabilities",
        ]
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for row in predictions:
            writer.writerow({key: json.dumps(row[key], ensure_ascii=False) if key == "probabilities" else row.get(key, "") for key in fieldnames})
    for task, task_metrics in metrics.items():
        write_confusion_csv(args.output_dir / f"confusion_{task}.csv", task_metrics)
    with (args.output_dir / "threshold_analysis.csv").open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(report["threshold_analysis"][0]))
        writer.writeheader()
        writer.writerows(report["threshold_analysis"])

    print(f"Evaluated {len(predictions)} samples. Results written to: {args.output_dir.resolve()}")
    for task, task_metrics in metrics.items():
        print(f"{task}: accuracy={task_metrics['accuracy']:.4f}, macro_f1={task_metrics['macro_f1']:.4f}")


if __name__ == "__main__":
    main()

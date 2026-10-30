"""Train a LoRA adapter on gai's training set (chat JSON Lines).

gai runs this in a container with the GPU: /gai/train.jsonl is the data,
and the environment says what to train (GAI_BASE_MODEL, GAI_OUT,
GAI_EPOCHS, GAI_LR, GAI_LORA_RANK). The adapter is saved to GAI_OUT,
which vLLM serves with --lora-modules.
"""

import json
import os

import torch
from datasets import Dataset
from peft import LoraConfig
from transformers import AutoTokenizer
from trl import SFTConfig, SFTTrainer


def rows(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                ex = json.loads(line)
                # Tool calls stay as text: chat templates differ in how they
                # take them, and every template takes plain messages.
                msgs = []
                for m in ex["messages"]:
                    content = m.get("content") or ""
                    if m.get("tool_calls"):
                        calls = [c["function"] for c in m["tool_calls"]]
                        content = (content + "\n" + json.dumps(calls)).strip()
                    role = "user" if m["role"] == "tool" else m["role"]
                    msgs.append({"role": role, "content": content})
                yield {"messages": msgs}


def main():
    base = os.environ["GAI_BASE_MODEL"]
    out = os.environ["GAI_OUT"]
    data = Dataset.from_list(list(rows("/gai/train.jsonl")))
    print(f"training {base} on {len(data)} conversations", flush=True)
    tokenizer = AutoTokenizer.from_pretrained(base)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    rank = int(os.environ.get("GAI_LORA_RANK", "16"))
    config = SFTConfig(
        output_dir="/tmp/gai-train",
        num_train_epochs=float(os.environ.get("GAI_EPOCHS", "3")),
        learning_rate=float(os.environ.get("GAI_LR", "2e-4")),
        per_device_train_batch_size=1,
        gradient_accumulation_steps=8,
        logging_steps=5,
        save_strategy="no",
        bf16=torch.cuda.is_available() and torch.cuda.is_bf16_supported(),
        report_to=[],
    )
    trainer = SFTTrainer(
        model=base,
        args=config,
        train_dataset=data,
        processing_class=tokenizer,
        peft_config=LoraConfig(
            r=rank,
            lora_alpha=2 * rank,
            lora_dropout=0.05,
            target_modules="all-linear",
            task_type="CAUSAL_LM",
        ),
    )
    trainer.train()
    trainer.model.save_pretrained(out)
    tokenizer.save_pretrained(out)
    print(f"saved the adapter to {out}", flush=True)


if __name__ == "__main__":
    main()

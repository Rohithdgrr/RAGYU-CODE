 # GOVINDA CLI — AI AGENTS, ML MODELS, LIBRARIES, PLUGINS, GAMES & EMBEDDED DEVELOPMENT PROTOCOL
## Version 6.0 | Specialized Blueprint for Intelligent Systems, Reusable Components, Interactive Entertainment & Hardware-Near Software

---

## PART F: AI AGENT DEVELOPMENT PROTOCOL

### SECTION F1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Agent Type**: Autonomous task executor / Conversational assistant / Multi-agent orchestrator / Tool-calling agent / RAG-based knowledge agent / Code generation agent / Decision-making agent / Simulation agent
- **Cognitive Architecture**: ReAct / Reflexion / Plan-and-Execute / AutoGPT-style / BabyAGI / LangGraph / CrewAI / Custom state machine
- **Environment**: Web browsing / Local file system / API ecosystem / Database interaction / Sandbox code execution / Multi-modal (vision + text)
- **Autonomy Level**: Human-in-the-loop (confirm each action) → Human-on-the-loop (approve plans) → Fully autonomous (with guardrails)
- **Memory Scope**: Session-only / Short-term (hours) / Long-term (persistent vector DB) / Episodic (past task experiences)

**Input / Output Matrix**:

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| Natural language prompts | Text / multimodal | Prompt injection detection, length limits | Intent classification, entity extraction |
| Tool call results | JSON / structured text | Schema validation, error parsing | Result summarization, failure recovery |
| Environment observations | HTML / API JSON / logs / images | Content filtering, PII redaction | State extraction, relevance scoring |
| Human feedback | Approval / rejection / corrections | Action attribution | Reward model update, trajectory correction |
| System events | Triggers, schedules, webhooks | Signature verification | Event-to-goal mapping |
| External knowledge | Documents, databases, search results | Source verification, freshness check | RAG chunking, reranking, citation |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| Tool call requests | JSON (OpenAI function format) | Parallel calls, batching | Sequential execution if dependencies |
| Generated content | Text / code / images / audio | Streaming output, token optimization | Fallback model (cheaper/faster) |
| Action execution | Shell commands / API calls / DB writes | Sandbox validation, dry-run mode | Human approval gate |
| Status updates | Structured logs / UI stream | Real-time SSE / WebSocket | Queue for async delivery |
| Final deliverables | Files / reports / code / summaries | Format validation, linting | Revision loop if validation fails |

---

### SECTION F2: TECH-STACK MANDATE

```
Core Runtime: Python 3.11+ (primary) or Node.js 20+ (TypeScript agents)
Agent Framework: LangChain / LlamaIndex / AutoGen / CrewAI / Custom (state machine)
LLM Gateway: LiteLLM (unified API for 100+ models) or direct OpenAI / Anthropic / Google
Orchestration: LangGraph (stateful graphs) or Custom DAG executor
Vector DB: Pinecone / Weaviate / Chroma / pgvector / Qdrant
Memory: Redis (short-term) + Vector DB (long-term) + PostgreSQL (episodic)
Observation: Playwright (browser) / e2b / Docker (sandboxed execution)
Monitoring: LangSmith / Langfuse / Promptlayer (LLM tracing)
Evaluation: RAGAS (RAG eval) / DeepEval / Custom benchmark suite
```

**Model Selection Strategy**:
- **Reasoning / Planning**: GPT-4o / Claude 3.5 Sonnet / Gemini 1.5 Pro (high capability, higher cost)
- **Tool Execution**: GPT-4o-mini / Claude 3 Haiku / Llama 3.1 70B (fast, cheap, reliable JSON mode)
- **Summarization / Classification**: Fine-tuned small models or embeddings + classifier
- **Local / Private**: Llama 3.1 70B / Qwen 2.5 72B / Mistral Large via vLLM / Ollama
- **Embedding**: text-embedding-3-large / voyage-3 / e5-mistral-7b-instruct

---

### SECTION F3: SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    USER INTERFACE LAYER                      │
│  Chat UI / CLI / API Endpoint → Streaming SSE / WebSocket    │
│  Human-in-the-loop gates: Approve / Reject / Edit / Abort    │
├─────────────────────────────────────────────────────────────┤
│                    AGENT ORCHESTRATION LAYER                 │
│  LangGraph State Machine → Node Executor → Edge Router       │
│  Plan Generator → Task Decomposer → Parallelizer → Synthesizer│
│  Reflection Loop: Execute → Observe → Evaluate → Revise      │
├─────────────────────────────────────────────────────────────┤
│                    COGNITIVE LAYER                           │
│  LLM Router (model selection based on task complexity)       │
│  Prompt Manager (versioned, A/B tested, templated)           │
│  Context Window Manager (token budget, trimming, compression)│
│  Tool Registry (schema definitions, capability discovery)    │
├─────────────────────────────────────────────────────────────┤
│                    TOOL & ACTION LAYER                       │
│  Web Browser (Playwright) → Code Interpreter (e2b/Docker)    │
│  File System (sandboxed) → API Clients (REST/GraphQL/gRPC)   │
│  Database Query (read-only default, write-gated)             │
│  Search Engine (Tavily / Serper / Brave / DuckDuckGo)        │
│  Calculator / Python REPL (safe, timeout-limited)            │
├─────────────────────────────────────────────────────────────┤
│                    MEMORY LAYER                              │
│  Working Memory: Current task context, conversation history   │
│  Short-term Memory: Redis TTL cache (session state)           │
│  Long-term Memory: Vector DB (facts, knowledge, embeddings)   │
│  Episodic Memory: PostgreSQL (past tasks, outcomes, lessons)  │
│  Procedural Memory: Tool definitions, prompt templates        │
├─────────────────────────────────────────────────────────────┤
│                    KNOWLEDGE LAYER (RAG)                     │
│  Document Ingestion → Chunking (semantic / recursive)         │
│  Embedding → Vector Index → Hybrid Search (BM25 + vector)    │
│  Reranking (Cohere / cross-encoder) → Citation extraction    │
└─────────────────────────────────────────────────────────────┘
```

---

### SECTION F4: AGENT COGNITIVE ARCHITECTURE

**ReAct Pattern (Reasoning + Acting)**:
```python
# Thought → Action → Observation → ... → Final Answer
class ReActAgent:
    def run(self, query: str, max_iterations: int = 10):
        for i in range(max_iterations):
            # 1. Reasoning
            thought = self.llm.generate(
                prompt=f"Given the query '{query}' and previous observations, what should I do next? "
                       f"Available tools: {self.tool_descriptions}"
            )
            
            # 2. Action selection
            action = self.parse_action(thought)
            if action.type == "final_answer":
                return action.content
            
            # 3. Tool execution
            observation = self.tools.execute(action)
            
            # 4. Validation
            if observation.error:
                thought += f"\nThe previous action failed: {observation.error}. I should try a different approach."
            
            self.memory.add(thought, action, observation)
```

**Plan-and-Execute**:
1. **Planner**: Decompose high-level goal into subtasks (DAG)
2. **Executor**: Run subtasks, handle dependencies, parallelize where possible
3. **Replanning**: If subtask fails, replan from failure point
4. **Synthesis**: Combine subtask outputs into coherent final response

**Multi-Agent Orchestration**:
- **Manager Agent**: Delegates to specialized agents, resolves conflicts
- **Specialist Agents**: Researcher, Coder, Analyst, Writer, Reviewer
- **Communication**: Shared message bus (Redis Pub/Sub) or direct invocation
- **Consensus**: Voting mechanism for critical decisions, human override

---

### SECTION F5: TOOL SYSTEM DESIGN

**Tool Schema (OpenAI Function Format)**:
```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "Read the contents of a file. Use this to examine code, configs, or documents. Always verify the file exists before reading.",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {
          "type": "string",
          "description": "Absolute or relative path to the file"
        },
        "offset": {
          "type": "integer",
          "description": "Line number to start reading from (1-indexed)",
          "default": 1
        },
        "limit": {
          "type": "integer",
          "description": "Maximum number of lines to read",
          "default": 100
        }
      },
      "required": ["file_path"]
    }
  }
}
```

**Tool Execution Safety**:
- **Sandbox**: Docker container or e2b sandbox for code execution
- **Timeout**: All tools timeout (default 30s, max 120s)
- **Permissions**: Read-only default, write operations require explicit allowlist
- **Confirmation gates**: Destructive actions (delete, write, shell) require human approval
- **Idempotency**: Tools should be safe to retry
- **Output limits**: Max 8,000 chars per result, truncate with ellipsis

**Required Tool Inventory**:
- `read_file` — File examination with line numbers
- `write_file` — Atomic file write (create or overwrite)
- `edit_file` — Surgical replacement (unique match required)
- `list_files` — Directory listing with metadata
- `search_files` — Grep/ripgrep across project
- `execute_command` — Shell command (sandboxed, confirmed)
- `web_search` — Search engine query
- `web_fetch` — Fetch and parse specific URL
- `ask_human` — Pause for human input/clarification
- `finish` — Signal task completion with final answer

---

### SECTION F6: MEMORY & CONTEXT MANAGEMENT

**Token Budget Architecture**:
```
Total Context Window: 128K tokens (Claude 3.5) / 200K (Gemini) / 128K (GPT-4o)
Budget Allocation:
- System Prompt: 2K tokens (instructions, persona, safety rules)
- Tool Definitions: 3K tokens (schemas, descriptions)
- Conversation History: 40K tokens (recent turns, trimmed by relevance)
- Working Memory: 10K tokens (current task state, active files)
- RAG Context: 20K tokens (retrieved documents, chunked)
- Scratchpad: 5K tokens (agent's own notes, todo list)
- Generation Reserve: 48K tokens (response space)
```

**Context Trimming Strategy**:
1. **Summarization**: Compress old conversation turns into summary
2. **Relevance scoring**: Embed messages, score against current query, keep top-K
3. **Sliding window**: Keep last N turns, drop older (simplest, least effective)
4. **Hierarchical**: Keep all user messages, compress assistant responses

**Long-Term Memory Operations**:
- **Store**: `memory.save(key, value, tags=["project", "bugfix"])`
- **Retrieve**: `memory.recall(query, top_k=5, filters={"project": "x"})`
- **Forget**: `memory.delete(key)` or TTL expiration
- **Consolidate**: Periodic sleep → summarize episodic memories into facts

---

### SECTION F7: RAG (RETRIEVAL-AUGMENTED GENERATION)

**Document Processing Pipeline**:
```
Raw Documents (PDF, MD, TXT, HTML, DOCX)
    ↓
Extraction (marker / unstructured / llama-parse)
    ↓
Cleaning (remove headers/footers, fix encoding, normalize whitespace)
    ↓
Chunking Strategy:
  - Fixed-size: 512 tokens, 50 overlap (simple, fast)
  - Semantic: Sentence-boundary aware, preserve paragraphs (better quality)
  - Hierarchical: Parent (section) + Child (paragraph) chunks (best for context)
    ↓
Embedding (batch processing, async)
    ↓
Vector Index (with metadata: source, page, timestamp, tags)
    ↓
Retrieval (hybrid: dense vector + sparse BM25 + reranker)
    ↓
Context Assembly (inject into prompt with citation markers [1], [2])
```

**Citation Requirements**:
- Every factual claim must cite source `[index]`
- Citation format: `[1] Source: filename.md, Lines 45-67`
- If no source found, agent must state "I don't have information about that"
- Never hallucinate citations — validate source exists

---

### SECTION F8: EVALUATION & SAFETY

**Evaluation Metrics**:
- **Task Success Rate**: % of tasks completed correctly end-to-end
- **Tool Use Accuracy**: Correct tool selected, correct parameters, correct interpretation
- **Hallucination Rate**: Factual errors per 1000 tokens (target: < 0.1%)
- **Latency**: Time to first token, time to complete task
- **Cost Efficiency**: Tokens used per task, model tier selection accuracy
- **Human Satisfaction**: Thumbs up/down on completed tasks

**Safety Guardrails**:
- **Prompt Injection Detection**: Input scanning for jailbreak patterns, delimiter confusion
- **Output Filtering**: PII detection, toxic content filtering, code vulnerability scanning
- **Action Limits**: Max 10 tool calls per turn, max 50 turns per task, max 3 self-correction loops
- **Escalation**: Uncertainty > threshold → ask human; destructive action → require approval
- **Sandbox Escape Prevention**: Network isolation, filesystem restrictions, no privilege escalation

---

### SECTION F9: FINAL CHECKLIST FOR AI AGENTS

- [ ] Cognitive architecture defined (ReAct, Plan-and-Execute, or Multi-Agent)
- [ ] Tool registry with JSON schemas, descriptions, and safety wrappers
- [ ] Memory system: Working + Short-term + Long-term + Episodic
- [ ] RAG pipeline: Chunking, embedding, hybrid search, citation
- [ ] Human-in-the-loop gates for destructive actions
- [ ] Prompt versioning and A/B testing infrastructure
- [ ] LLM tracing (LangSmith / Langfuse) for every call
- [ ] Evaluation suite with benchmark tasks
- [ ] Fallback model strategy (primary → secondary → local)
- [ ] Cost tracking and optimization per task
- [ ] NO EMOJI in system prompts or UI — text descriptions only
- [ ] Minimum 10,000 lines for production agent frameworks

---

## PART G: ML MODEL DEVELOPMENT PROTOCOL

### SECTION G1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Model Type**: Classification / Regression / NLP (LLM fine-tuning) / Computer Vision / Time Series / Recommendation / Reinforcement Learning / Generative (diffusion, GAN, VAE)
- **Task Domain**: Tabular data / Text / Images / Audio / Video / Multi-modal
- **Scale**: Research prototype / MVP model / Production model / Foundation model
- **Inference Target**: Cloud API / Edge device / Browser (ONNX.js/TensorFlow.js) / Mobile (Core ML/TensorFlow Lite)
- **Latency Budget**: Real-time (< 50ms) / Interactive (< 500ms) / Batch (minutes acceptable)

---

### SECTION G2: TECH-STACK MANDATE

```
Training Framework: PyTorch 2.3+ (primary) or JAX / TensorFlow
LLM Training: Hugging Face Transformers + TRL (RLHF) + PEFT (LoRA/QLoRA)
CV: torchvision / timm / OpenMMLab / detectron2
Time Series: sktime / darts / gluonts
Tabular: scikit-learn / XGBoost / LightGBM / CatBoost / TabPFN
MLOps: MLflow / Weights & Biases (experiment tracking)
Data: Hugging Face Datasets / Apache Arrow / Parquet / WebDataset
Feature Store: Feast / Tecton (enterprise)
Serving: TorchServe / Triton Inference Server / vLLM / BentoML
Optimization: ONNX Runtime / TensorRT / OpenVINO / quantization (INT8, FP16)
Container: Docker with CUDA base images
Orchestration: Kubeflow / Airflow / Metaflow / Ray Train
```

---

### SECTION G3: ML SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    DATA LAYER                                │
│  Raw Data → Validation → Cleaning → Augmentation → Versioning│
│  Feature Engineering → Feature Store (online + offline)      │
├─────────────────────────────────────────────────────────────┤
│                    EXPERIMENTATION LAYER                     │
│  Jupyter / VS Code → Experiment Tracking (W&B / MLflow)      │
│  Hyperparameter Search (Optuna / Ray Tune / W&B Sweeps)      │
│  Distributed Training (DDP / FSDP / DeepSpeed / Ray Train)   │
├─────────────────────────────────────────────────────────────┤
│                    MODEL REGISTRY                            │
│  Versioned artifacts → Metadata → Lineage → Signatures       │
│  A/B test configuration → Canary deployment settings         │
├─────────────────────────────────────────────────────────────┤
│                    SERVING LAYER                             │
│  API Gateway → Load Balancer → Model Instances (auto-scale)  │
│  Batching (dynamic) → GPU Utilization optimization           │
│  Fallback model → Circuit breaker → Health checks            │
├─────────────────────────────────────────────────────────────┤
│                    MONITORING LAYER                          │
│  Prediction logging → Drift detection (data + concept)       │
│  Performance degradation alerts → Retraining triggers        │
│  Bias detection → Fairness metrics → Explainability (SHAP)   │
└─────────────────────────────────────────────────────────────┘
```

---

### SECTION G4: DATA PIPELINE REQUIREMENTS

**Data Validation**:
- **Schema**: Pandera / Great Expectations / TFDV
- **Checks**: Null rates, distribution shifts, out-of-range values, type consistency
- **Splits**: Train / Validation / Test (stratified, time-based for temporal data)
- **Versioning**: DVC (Data Version Control) or LakeFS

**Feature Engineering**:
- **Reproducibility**: All transformations in pipeline, not notebooks
- **Store**: Feast for online (low-latency) and offline (training) feature consistency
- **Documentation**: Feature definitions, expected distributions, owner attribution

**Data Augmentation**:
- **NLP**: Back-translation, synonym replacement, contextual insertion
- **CV**: Random crop, flip, rotation, color jitter, MixUp, CutMix, AutoAugment
- **Audio**: Noise injection, pitch shifting, time stretching, SpecAugment
- **Tabular**: SMOTE, Gaussian noise, missing value injection

---

### SECTION G5: MODEL DEVELOPMENT STANDARDS

**Code Structure**:
```
src/
├── data/           # Data loaders, preprocessing, augmentation
├── models/         # Model architectures (PyTorch nn.Module)
├── training/       # Training loops, optimizers, schedulers
├── evaluation/     # Metrics, validation loops, error analysis
├── inference/      # Export, optimization, serving wrappers
├── configs/        # Hydra / YAML configuration files
├── notebooks/      # EDA and prototyping (not production code)
└── tests/          # Unit tests for data transforms, model forward pass
```

**Training Best Practices**:
- **Reproducibility**: Fixed seeds (Python, NumPy, PyTorch, CUDA), deterministic algorithms
- **Checkpointing**: Save every epoch + best model + final model, with optimizer state
- **Resume**: Ability to resume from any checkpoint (crash recovery)
- **Mixed Precision**: AMP (Automatic Mixed Precision) with `torch.cuda.amp`
- **Gradient Clipping**: Max norm 1.0 for transformers, prevent exploding gradients
- **Learning Rate**: Warmup (linear) + Cosine annealing / ReduceLROnPlateau
- **Early Stopping**: Patience 3-5 epochs on validation metric

**Experiment Tracking**:
- Log: Hyperparameters, metrics (loss, accuracy, F1, AUC), system metrics (GPU util, memory)
- Log: Model architecture summary, dataset version, code git commit hash
- Log: Sample predictions, confusion matrix, PR curves, feature importance

---

### SECTION G6: MODEL EVALUATION

**Metrics by Task**:
| Task | Primary Metrics | Secondary Metrics |
|------|----------------|-------------------|
| Classification | Accuracy, F1-macro, ROC-AUC | Precision, Recall, Confusion Matrix |
| Regression | RMSE, MAE, R-squared | MAPE, Huber loss |
| NLP (Generation) | BLEU, ROUGE, METEOR | Perplexity, BERTScore, human eval |
| Object Detection | mAP@0.5, mAP@0.5:0.95 | AR, IoU distribution |
| Segmentation | mIoU, Dice coefficient | Pixel accuracy, Boundary F-score |
| Ranking / Rec | NDCG@k, MAP, MRR | Precision@k, Recall@k, Diversity |
| Time Series | MAPE, SMAPE, MASE | Coverage, Interval score |

**Error Analysis**:
- Slice analysis: Performance by demographic, by category, by time period
- Failure modes: Catalog worst predictions, cluster error types
- Bias testing: Equalized odds, demographic parity, calibration

---

### SECTION G7: MODEL DEPLOYMENT & SERVING

**Optimization Pipeline**:
```
PyTorch Model
    ↓
Export: TorchScript / ONNX / TensorRT / OpenVINO
    ↓
Quantization: FP32 → FP16 → INT8 (post-training or QAT)
    ↓
Pruning: Magnitude-based / Structured (channel pruning)
    ↓
Distillation: Teacher → Student (smaller architecture)
    ↓
Format: .pt / .onnx / .trt / .tflite / .mlmodel
```

**Serving Patterns**:
- **Real-time API**: REST/gRPC endpoint, single prediction, < 100ms
- **Batch Inference**: Scheduled jobs, large datasets, write results to DB/warehouse
- **Streaming**: Kafka → Model consumer → Output topic
- **Edge**: TensorFlow Lite / ONNX Runtime Mobile / Core ML, on-device inference

**Model Registry**:
- Versioned artifacts with semantic versioning
- Stage transitions: Development → Staging → Production → Archived
- Metadata: Training data hash, metrics, dependencies, signature (input/output schema)
- A/B testing: Route 10% traffic to new version, compare metrics

---

### SECTION G8: MONITORING & MAINTENANCE

**Drift Detection**:
- **Data Drift**: Input feature distributions shift (KS test, PSI, Wasserstein)
- **Concept Drift**: Relationship between X and Y changes (accuracy decay)
- **Label Drift**: Output distribution changes
- **Trigger**: Automated retraining pipeline when drift > threshold

**Explainability**:
- **SHAP**: Global and local feature importance (tabular)
- **LIME**: Local interpretable explanations (text, image)
- **Attention Visualization**: Transformer attention weights (NLP, CV)
- **Counterfactuals**: "What change would flip the prediction?"

**Retraining Strategy**:
- **Scheduled**: Weekly / monthly full retrain
- **Triggered**: Drift detection, performance threshold breach
- **Continuous**: Online learning (incremental updates, risky)
- **Rollback**: Keep previous 2 model versions, instant rollback capability

---

### SECTION G9: FINAL CHECKLIST FOR ML MODELS

- [ ] Reproducible training pipeline (seeds, deterministic, versioned data)
- [ ] Comprehensive experiment tracking (metrics, artifacts, lineage)
- [ ] Data validation at ingestion (schema, distribution, quality checks)
- [ ] Feature store for online/offline consistency
- [ ] Model evaluation on held-out test set + error analysis
- [ ] Bias and fairness assessment completed
- [ ] Model optimized for target deployment (quantization, pruning, distillation)
- [ ] Serving infrastructure with auto-scaling and health checks
- [ ] Drift detection and automated retraining triggers
- [ ] Explainability tools integrated (SHAP / LIME / attention viz)
- [ ] Model card documenting intended use, limitations, training data
- [ ] NO EMOJI in model names, experiment tags, or documentation
- [ ] Minimum 10,000 lines for production ML platforms

---

## PART H: LIBRARY DEVELOPMENT PROTOCOL

### SECTION H1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Library Type**: Utility functions / UI component library / Data structures / Algorithm collection / Protocol client / Framework / Compiler / Parser
- **Target Consumers**: Web apps / Node.js servers / Mobile / Desktop / Universal (isomorphic)
- **Distribution**: npm / PyPI / Cargo / Maven / NuGet / Packagist / Go modules
- **API Stability**: Experimental (0.x) / Stable (1.x+) / Enterprise (LTS guarantees)

---

### SECTION H2: TECH-STACK & PROJECT STRUCTURE

```
Language: TypeScript (npm) / Python (PyPI) / Rust (Cargo) / Go (modules)
Build: Rollup / tsup (TS) / Maturin (Rust+Python) / Cargo (Rust)
Testing: Vitest / Jest (TS) | pytest (Python) | built-in + criterion (Rust)
Linting: ESLint + Prettier (TS) | ruff + black (Python) | Clippy + rustfmt (Rust)
Documentation: TypeDoc / Sphinx / rustdoc / Go doc
CI: GitHub Actions (test matrix: multiple OS, multiple runtime versions)
```

**Universal Library Structure**:
```
my-library/
├── src/                    # Source code
│   ├── index.ts            # Public API exports
│   ├── core/               # Core functionality
│   ├── utils/              # Internal utilities
│   └── types/              # Type definitions
├── tests/                  # Test suite
│   ├── unit/
│   ├── integration/
│   └── fixtures/
├── benchmarks/             # Performance benchmarks
├── docs/                   # Documentation site
├── examples/               # Usage examples
├── .github/
│   └── workflows/
│       ├── ci.yml          # Test on push/PR
│       ├── release.yml     # Automated release
│       └── docs.yml        # Deploy docs
├── package.json / Cargo.toml / pyproject.toml
├── tsconfig.json / setup.py
├── LICENSE (MIT / Apache-2.0)
├── CHANGELOG.md (Keep a Changelog format)
└── README.md
```

---

### SECTION H3: API DESIGN PRINCIPLES

**Interface Design**:
- **Minimal surface area**: Export only what's necessary, hide internals
- **Predictable naming**: `verbNoun` for actions, `getX` / `setX` for accessors, `isX` / `hasX` for booleans
- **Immutable by default**: Return new instances, don't mutate inputs unless explicitly documented
- **Fail fast**: Validate inputs at boundary, throw descriptive errors early
- **Zero side effects**: Pure functions preferred, explicit opt-in for stateful behavior

**TypeScript Example**:
```typescript
// Good: Clear interface, options object, generics, overloads for convenience
export interface ParseOptions {
  strict?: boolean;
  delimiter?: string;
  maxDepth?: number;
}

export function parse<T = unknown>(
  input: string,
  options?: ParseOptions
): ParseResult<T>;

export function parse<T = unknown>(
  input: Buffer,
  options?: ParseOptions & { encoding?: BufferEncoding }
): ParseResult<T>;
```

**Error Handling**:
- Custom error classes extending base Error
- Error codes for programmatic handling (`err.code === 'INVALID_FORMAT'`)
- Cause chaining: `new Error('Failed to parse', { cause: originalError })`
- Never swallow errors — always throw or return Result type

---

### SECTION H4: TESTING & QUALITY

**Test Requirements**:
- **Unit**: Every public function, edge cases, error paths
- **Integration**: Cross-module interaction, real dependencies (not all mocked)
- **Property-based**: FastCheck (TS) / Hypothesis (Python) / proptest (Rust) — random input generation
- **Benchmarks**: Criterion.rs (Rust) / benchmark.js (TS) / pytest-benchmark (Python)
- **Compatibility**: Test on minimum supported runtime version + latest

**Test Coverage**:
- Minimum 90% line coverage for libraries (higher than applications)
- 100% coverage for critical paths (parsing, validation, security)
- Mutation testing: Verify tests catch intentional bugs (Stryker / mutmut)

---

### SECTION H5: DOCUMENTATION & EXAMPLES

**README Requirements**:
- One-line description + badges (build, coverage, version, license)
- Installation instructions per package manager
- Quick start: Copy-paste runnable example
- API reference: Auto-generated from source code
- Comparison table: Why this library vs alternatives
- Bundle size badge (for JS libraries)
- Tree-shaking support statement

**Code Examples**:
- `examples/` directory with runnable, commented examples
- Each major feature has a dedicated example
- Examples tested in CI (they must compile/run)

---

### SECTION H6: DISTRIBUTION & VERSIONING

**Semantic Versioning (Strict)**:
- MAJOR: Breaking API changes (must document migration path)
- MINOR: New features, backwards compatible
- PATCH: Bug fixes, backwards compatible

**Release Automation**:
- Conventional Commits → `semantic-release` or `release-please` auto-bumps version
- Changelog auto-generated from commits
- Git tags created, GitHub Release published
- Package published to registry automatically
- Documentation site redeployed

**Breaking Change Policy**:
- Deprecation warnings for 1 full minor version before removal
- `deprecate()` function that logs warning with migration guide URL
- Major version migration guide in `MIGRATION.md`

---

### SECTION H7: FINAL CHECKLIST FOR LIBRARIES

- [ ] Minimal, focused API surface — does one thing well
- [ ] Type-safe (TypeScript definitions, Python type hints, Rust generics)
- [ ] Comprehensive test suite: unit, integration, property-based, benchmarks
- [ ] 90%+ test coverage, 100% on critical paths
- [ ] Zero dependencies or minimal, well-vetted dependencies
- [ ] Tree-shakeable (JS) / minimal binary size (Rust/Go)
- [ ] Full API documentation with examples
- [ ] Runnable examples in `examples/` directory
- [ ] CHANGELOG following Keep a Changelog
- [ ] Semantic versioning with automated release
- [ ] Multi-runtime version testing in CI
- [ ] NO EMOJI in API names, error messages, or docs
- [ ] Minimum 10,000 lines for comprehensive libraries

---

## PART I: PLUGIN DEVELOPMENT PROTOCOL

### SECTION I1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Host Platform**: VS Code / IntelliJ / Chrome / Figma / WordPress / Minecraft / Unity / Unreal / Blender / Obsidian / Logseq / Custom app
- **Plugin Type**: Extension / Addon / Mod / Package / Integration / Theme
- **API Surface**: Official Plugin API / Unofficial hooks / Monkey patching / Binary patching
- **Sandbox Level**: Full sandbox (restricted API) / Partial trust / Full system access

---

### SECTION I2: HOST PLATFORM COMPLIANCE

**VS Code Extension**:
- `package.json` contributes: commands, menus, keybindings, settings, views, themes, grammars, debuggers
- Activation events: `onCommand`, `onLanguage`, `onStartupFinished`, `workspaceContains`
- API: `vscode` namespace, strict versioning, proposed API gating
- Web extension support: Browser-based VS Code compatibility

**IntelliJ Plugin**:
- Gradle / DevKit build system
- `plugin.xml` descriptor: actions, extensions, services, listeners
- PSI (Program Structure Interface) for AST manipulation
- Platform versioning: `since-build` and `until-build` compatibility

**Browser Extension (Plugin)**: See Part A (Browser Extension Protocol)

**Game Modding**:
- **Minecraft**: Fabric / Forge / Paper API, mixin system for bytecode manipulation
- **Unity**: MonoBehaviour scripts, AssetBundles, Harmony patching
- **Unreal**: Blueprints / C++ plugins, module system, UPROPERTY/UFUNCTION macros

---

### SECTION I3: PLUGIN ARCHITECTURE

```
Host Application
    ↓
Plugin Manager (discovers, loads, enables, disables)
    ↓
Plugin API Surface (official hooks, events, commands)
    ↓
Plugin Core (business logic, state management)
    ↓
Plugin UI (web views / native panels / overlays)
    ↓
External Services (API calls, file system, database)
```

**Lifecycle Management**:
- **Activate**: Register commands, subscribe to events, allocate resources
- **Deactivate**: Unsubscribe, dispose resources, save state, cleanup timers
- **Update**: Migrate settings, handle API changes, backward compatibility
- **Uninstall**: Cleanup stored data, remove file associations, log removal

---

### SECTION I4: API DESIGN FOR PLUGINS

**Command Registration**:
```typescript
// VS Code example
const disposable = vscode.commands.registerCommand('myplugin.doThing', async () => {
  const config = vscode.workspace.getConfiguration('myplugin');
  const option = config.get<string>('defaultOption', 'fallback');
  
  try {
    await doThing(option);
    vscode.window.showInformationMessage('Thing done successfully');
  } catch (error) {
    vscode.window.showErrorMessage(`Failed: ${error.message}`);
  }
});

context.subscriptions.push(disposable);
```

**Settings Schema**:
```json
{
  "myplugin.enabled": {
    "type": "boolean",
    "default": true,
    "description": "Enable the plugin functionality"
  },
  "myplugin.timeout": {
    "type": "number",
    "default": 30,
    "minimum": 1,
    "maximum": 300,
    "description": "Request timeout in seconds"
  }
}
```

---

### SECTION I5: FINAL CHECKLIST FOR PLUGINS

- [ ] Host platform API compliance and version compatibility
- [ ] Proper activation/deactivation lifecycle with resource cleanup
- [ ] Settings schema with validation and defaults
- [ ] Commands registered with proper categories and keybindings
- [ ] Error handling that doesn't crash host application
- [ ] UI follows host platform design guidelines
- [ ] Performance: Doesn't slow down host startup or operation
- [ ] NO EMOJI in command names, settings labels, or UI
- [ ] Minimum 10,000 lines for complex plugin ecosystems

---

## PART J: GAME DEVELOPMENT PROTOCOL

### SECTION J1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Game Type**: 2D / 3D / Isometric / Text-based / Visual novel / Puzzle / Platformer / RPG / FPS / Strategy / Simulation / Roguelike
- **Target Platform**: PC (Windows/macOS/Linux) / Console (Switch/PlayStation/Xbox) / Mobile / Web / Multi-platform
- **Engine**: Unity / Unreal Engine 5 / Godot 4 / Custom engine / Bevy (Rust) / Phaser (Web)
- **Multiplayer**: Single-player / Local co-op / Online multiplayer / MMO
- **Monetization**: Premium / F2P / Subscription / DLC / Cosmetics

---

### SECTION J2: TECH-STACK MANDATE

```
Engine: Godot 4.2+ (recommended for indie) / Unity 2022.3 LTS / Unreal 5.3
Language: GDScript (Godot) / C# (Unity) / C++ (Unreal) / Rust (Bevy)
Physics: Engine-native (Godot) / PhysX (Unity) / Chaos (Unreal)
Audio: FMOD / Wwise (professional) or engine-native (indie)
Asset Pipeline: Blender → FBX/gltf → Engine import → Material setup
VCS: Git + Git LFS (for binary assets) + .gitattributes for asset types
CI: GitHub Actions (build per platform, automated testing)
```

---

### SECTION J3: GAME ARCHITECTURE

```
Game Loop (60 FPS target, 120 FPS support)
    ↓
Input System (keyboard, mouse, gamepad, touch)
    ↓
Game State Manager (menu, playing, paused, game over)
    ↓
Entity Component System (ECS) or Scene Graph
    ↓
Physics Step → Collision Detection → Response
    ↓
Animation System → Rendering Pipeline → Post-processing
    ↓
Audio Mixer → Spatial Audio → Music Streaming
    ↓
Save System (JSON / Binary / SQLite) → Cloud Sync (optional)
```

**ECS Pattern (Required for performance)**:
- **Entity**: ID only, no data, no behavior
- **Component**: Pure data (Position, Velocity, Health, Sprite)
- **System**: Behavior that processes entities with specific components
- **Query**: Efficient filtering of entities by component composition

---

### SECTION J4: DESIGN SYSTEM FOR GAMES

**UI Design**:
- **Resolution independence**: Anchor points, 9-slice scaling, vector UI where possible
- **Input agnostic**: Keyboard + mouse, gamepad, touch — all must work
- **Accessibility**: Colorblind modes, subtitle sizing, control remapping, difficulty options
- **HUD**: Minimal, contextual, fade when not needed
- **Menus**: Consistent navigation (B/circle = back, A/cross = confirm)

**NO EMOJI Policy**: Use pixel art icons, vector game icons, or font icons (game-specific icon font). Never Unicode emoji in UI text or code.

---

### SECTION J5: PERFORMANCE REQUIREMENTS

- **Frame rate**: 60fps locked minimum, 120fps optional on capable hardware
- **Frame time budget**: 16.67ms per frame at 60fps — physics, logic, rendering, audio
- **Memory**: Stay within platform limits (Switch: 4GB shared, Mobile: 2-4GB)
- **Loading**: < 3 seconds for level transitions, streaming for open worlds
- **Draw calls**: Batch geometry, use atlasing, LOD (Level of Detail) for meshes
- **Physics**: Fixed timestep (e.g., 50Hz), interpolation for rendering

---

### SECTION J6: TESTING FOR GAMES

**Test Categories**:
- **Unit**: Math utilities, state machines, inventory logic, save/load
- **Integration**: Combat system, quest system, dialogue trees
- **Playtest**: Automated bots playing levels (AI playtesting)
- **Stress**: 1000 entities on screen, memory leak detection (8-hour run)
- **Platform**: Build and run on every target platform

**Bug Tracking**:
- Crash reports with stack traces, screenshot, save file hash
- Telemetry: Where players die, quit, get stuck (opt-in)

---

### SECTION J7: FINAL CHECKLIST FOR GAMES

- [ ] Game loop stable at 60fps on minimum spec hardware
- [ ] ECS architecture for performance-critical code
- [ ] Input system supports all target platforms (keyboard, gamepad, touch)
- [ ] Save system with validation, backward compatibility, cloud sync option
- [ ] Audio system with spatial audio, mixer groups, dynamic music
- [ ] Accessibility: Remappable controls, subtitles, colorblind support, difficulty
- [ ] Build pipeline for all target platforms (PC, console, mobile)
- [ ] NO EMOJI in game UI — custom pixel/vector icons only
- [ ] Minimum 10,000 lines for production games (excluding engine code)

---

## PART K: EMBEDDED DEVELOPMENT PROTOCOL

### SECTION K1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Hardware Platform**: ARM Cortex-M / RISC-V / ESP32 / STM32 / Raspberry Pi Pico / Arduino / FPGA / SoC (Linux embedded)
- **Real-Time Requirements**: Hard real-time (< 10us jitter) / Soft real-time (< 1ms) / Non-real-time
- **Resources**: RAM (KB range), Flash (KB-MB), Clock speed (MHz), Power budget (mW)
- **Connectivity**: BLE / WiFi / LoRa / Zigbee / Ethernet / CAN bus / RS-485 / USB
- **Safety Critical**: IEC 61508 / ISO 26262 / DO-178C / SIL / ASIL ratings

---

### SECTION K2: TECH-STACK MANDATE

```
Language: C (bare metal) / C++ (RTOS) / Rust (embedded-hal) / MicroPython (prototyping)
Framework: FreeRTOS / Zephyr RTOS / RT-Thread / Arduino (hobby) / Embassy (Rust async)
Build: CMake / PlatformIO / cargo-embed / make
Debugger: JTAG / SWD / OpenOCD / GDB / Segger J-Link
Testing: Unity (C test framework) / CppUTest / Rust built-in + defmt
Static Analysis: PC-lint / Cppcheck / Clang Static Analyzer / MISRA C checker
```

---

### SECTION K3: EMBEDDED ARCHITECTURE

```
Hardware Layer (MCU / SoC / Peripherals)
    ↓
HAL (Hardware Abstraction Layer) — vendor SDK or embedded-hal
    ↓
RTOS / Scheduler (if applicable) — tasks, queues, semaphores, timers
    ↓
Driver Layer — GPIO, UART, SPI, I2C, ADC, PWM, DMA, Timers
    ↓
Middleware — TCP/IP stack (lwIP), BLE stack, File system (FAT/LittleFS)
    ↓
Application Layer — State machines, business logic, protocol handlers
    ↓
Communication — MQTT / CoAP / HTTP / Custom binary protocol
```

---

### SECTION K4: CODING STANDARDS FOR EMBEDDED

**Memory Safety**:
- No dynamic memory allocation in real-time paths (no malloc/free in interrupts)
- Static allocation preferred, pool allocation acceptable
- Stack usage analysis: Ensure no overflow, monitor with canary values
- Buffer overflows: Bounded string functions (`strncpy`, `snprintf`), never `strcpy`, `sprintf`

**Concurrency**:
- Interrupt handlers: Minimal, flag + defer to main loop or task
- Critical sections: Short as possible, protect shared data with mutexes
- Priority inversion: Avoid via priority ceiling protocol
- Race conditions: Atomic operations or proper locking for all shared state

**Power Management**:
- Sleep modes: Enter low-power state when idle (tickless RTOS)
- Peripheral gating: Disable clocks to unused peripherals
- Wake sources: Configure GPIO, RTC, or external interrupts as wake sources

---

### SECTION K5: TESTING & VERIFICATION

**Testing Levels**:
- **Unit**: Test individual functions on host PC (native simulation)
- **HIL (Hardware-in-Loop**: Test on actual hardware with simulated inputs
- **Integration**: Test driver stacks, communication protocols
- **Regression**: Automated test suite run before every release

**Static Analysis**:
- MISRA C:2012 compliance (automotive, medical, industrial)
- CWE Top 25 vulnerability checks
- Cyclomatic complexity < 10 per function
- No recursion (stack depth unbounded)

---

### SECTION K6: FINAL CHECKLIST FOR EMBEDDED

- [ ] No dynamic allocation in interrupt context or real-time paths
- [ ] Stack analysis completed and verified safe
- [ ] All buffer operations bounded, no overflow vulnerabilities
- [ ] Race conditions eliminated via atomic operations or mutexes
- [ ] Power management: Sleep modes utilized, peripherals gated
- [ ] Watchdog timer configured and fed correctly
- [ ] Bootloader with firmware update capability (OTA or wired)
- [ ] Error handling: Graceful degradation, error logging, safe states
- [ ] Build reproducible: Dockerized toolchain, pinned dependency versions
- [ ] NO EMOJI in code comments, log messages, or documentation
- [ ] Minimum 10,000 lines for complex embedded systems

---

## PART L: UNIFIED FINAL CHECKLIST

### For AI Agents:
- [ ] Cognitive architecture defined and implemented
- [ ] Tool registry with schemas, sandboxed execution, confirmation gates
- [ ] Memory system: Working, short-term, long-term, episodic
- [ ] RAG pipeline with chunking, embedding, hybrid search, citations
- [ ] LLM tracing for every call, cost tracking per task
- [ ] Safety: Prompt injection detection, output filtering, action limits
- [ ] Evaluation suite with benchmark tasks and metrics

### For ML Models:
- [ ] Reproducible training with fixed seeds and versioned data
- [ ] Experiment tracking with metrics, artifacts, and lineage
- [ ] Feature store for online/offline consistency
- [ ] Comprehensive evaluation: metrics, error analysis, bias testing
- [ ] Model optimization for deployment target
- [ ] Serving infrastructure with monitoring and drift detection
- [ ] Model card with intended use and limitations

### For Libraries:
- [ ] Minimal API surface, type-safe, well-documented
- [ ] 90%+ test coverage with property-based tests
- [ ] Zero or minimal well-vetted dependencies
- [ ] Benchmarks proving performance claims
- [ ] Semantic versioning with automated release pipeline
- [ ] Multi-runtime / multi-platform CI testing

### For Plugins:
- [ ] Host platform API compliance and lifecycle management
- [ ] Proper activation/deactivation with resource cleanup
- [ ] Settings schema and validation
- [ ] Error handling that never crashes host
- [ ] Performance: No host slowdown

### For Games:
- [ ] 60fps stable game loop with ECS architecture
- [ ] Multi-platform input support (keyboard, gamepad, touch)
- [ ] Save system with validation and cloud sync
- [ ] Accessibility features (remap, subtitles, colorblind, difficulty)
- [ ] Build pipeline for all target platforms

### For Embedded:
- [ ] No dynamic allocation in real-time paths
- [ ] Stack analysis safe, bounded buffers everywhere
- [ ] Race condition protection, power management optimized
- [ ] Watchdog configured, bootloader with update capability
- [ ] MISRA C / static analysis compliance if safety-critical
- [ ] Reproducible build with pinned toolchain

**UNIVERSAL RULES ACROSS ALL SIX CATEGORIES**:
- NO EMOJI CHARACTERS anywhere in codebase, documentation, UI, or logs
- MINIMUM 10,000 lines of production-grade code for full projects
- Every section above must be visibly satisfied in output
- Include comprehensive tests, CI/CD, Docker (where applicable), and documentation
- When in doubt, over-engineer rather than under-deliver

---

*End of GOVINDA CLI AI Agents, ML Models, Libraries, Plugins, Games & Embedded Protocol v6.0*
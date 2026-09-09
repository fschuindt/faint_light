# faint_light test & benchmark pipeline.
#
# Every suite runs either on the benchmark host (default) or locally,
# selected with ON=:
#
#   make test                      regression suite on the remote (deploys first)
#   make test ON=local             same suite against a locally spawned server
#   make test-failing [ON=local]   known dense-field failures (informational)
#   make bench-full   [ON=local]   full 256-image benchmark + scenarios,
#                                  prints benchmark-style summary tables
#   make bench-smoke  [ON=local]   2 images/class plumbing check (short
#                                  timeout: hard fields report timeout)
#
#   DEPLOY=0 make test             skip the redeploy, test what's running
#   FLT_TIMEOUT=60 make test-failing   shorter per-case budget
#
# Remote runs measure the deployed docker container via cgroups (the exact
# method used for the published benchmark) and pull per-case results back to
# ./bench-out/.
# Local runs spawn target/release/faint-light and use /proc accounting —
# handy for development, but numbers are not comparable to the benchmark host.
#
# Setup:
#   make remote-setup        one-time bootstrap of a fresh benchmark host
#                            (indexes + full image set + symlink)
#   make fetch-indexes       local: index files into ./indexes (gitignored)
#   make fetch-bench-images  local: full 280-image set into testdata/bench-full/
#   make deploy              just deploy current code to the remote

# Remote benchmark host: a .remote.<name>.env file (format in README,
# "Deploying to a home server"). Defaults to DEPLOY_REMOTE from .local.env,
# the same default deploy.sh uses; override with REMOTE=<name>.
-include .local.env
REMOTE ?= $(subst ",,$(DEPLOY_REMOTE))
-include .remote.$(REMOTE).env

ON ?= remote
PY ?= python3
SSH = ssh -p $(REMOTE_SSH_PORT) -i $(REMOTE_SSH_KEY) $(REMOTE_SSH_USER)@$(REMOTE_SSH_HOST)
RUNNER = FLT_MODE=container FLT_CONTAINER=faint_light FLT_HOST=localhost:7222 \
         $(if $(FLT_TIMEOUT),FLT_TIMEOUT=$(FLT_TIMEOUT)) python3 scripts/tests/run_suite.py

DEPLOY ?= 1
DEPLOY_DEP = $(if $(filter 1,$(DEPLOY)),deploy)

# run a suite remotely, always pulling ./bench-out (per-case CSV/JSON) back,
# and preserving the suite's exit code
define REMOTE_RUN
	@rc=0; $(SSH) 'cd $(REMOTE_PATH) && $(RUNNER) $(1)' || rc=$$?; \
	mkdir -p bench-out; \
	rsync -az -e "ssh -p $(REMOTE_SSH_PORT) -i $(REMOTE_SSH_KEY)" \
	    $(REMOTE_SSH_USER)@$(REMOTE_SSH_HOST):$(REMOTE_PATH)/bench-out/ bench-out/ 2>/dev/null || true; \
	echo "per-case results in ./bench-out/"; exit $$rc
endef

.PHONY: deploy remote-setup build test test-failing bench-full bench-smoke \
        fetch-indexes fetch-bench-images

deploy:
	./deploy.sh deploy $(REMOTE)

remote-setup:
	$(SSH) 'cd $(REMOTE_PATH) && ./scripts/download_indexes.sh && \
	        mkdir -p ~/bench && \
	        [ -f ~/bench/images/manifest.json ] || BENCH_IMG_DIR=~/bench/images python3 scripts/benchmark/fetch_images.py && \
	        ln -sfn ~/bench/images testdata/bench-full'

build:
	cargo build --release -p fl-server

ifeq ($(ON),remote)

test: $(DEPLOY_DEP)
	$(call REMOTE_RUN,regression)

test-failing: $(DEPLOY_DEP)
	$(call REMOTE_RUN,known-failing)

bench-full: $(DEPLOY_DEP)
	$(call REMOTE_RUN,full)

bench-smoke: FLT_TIMEOUT ?= 10
bench-smoke: $(DEPLOY_DEP)
	$(call REMOTE_RUN,full --limit 2)

else  # ON=local

test: build
	$(PY) scripts/tests/run_suite.py regression

test-failing: build
	$(PY) scripts/tests/run_suite.py known-failing

bench-full: build
	$(PY) scripts/tests/run_suite.py full

bench-smoke: export FLT_TIMEOUT ?= 10
bench-smoke: build
	$(PY) scripts/tests/run_suite.py full --limit 2

endif

fetch-indexes:
	./scripts/download_indexes.sh

fetch-bench-images:
	BENCH_IMG_DIR=testdata/bench-full $(PY) scripts/benchmark/fetch_images.py

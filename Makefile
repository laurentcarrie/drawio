.PHONY: help all clean step play confluence

-include .env
export

YAML   ?= test.yaml
STEM   := $(basename $(YAML))
OUTDIR := sandbox-$(STEM)

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  all           Build all slides, GIF, MP4 and HTML (no Confluence upload)"
	@echo "  step          Regenerate a single slide: make step step=stepN.drawio [dirty=1]"
	@echo "  play          Play the generated MP4 with mpv"
	@echo "  confluence    Build everything and upload to Confluence"
	@echo "  clean         Remove the sandbox output directory"
	@echo "  help          Show this help message (default)"

all: $(OUTDIR)/$(STEM).mp4

$(OUTDIR)/$(STEM).mp4: $(YAML) test.drawio
	cargo run --manifest-path drawio-lc/Cargo.toml -- $(YAML) --no-confluence

step:
	cargo run --manifest-path drawio-lc/Cargo.toml -- $(YAML) --step $(step) --no-confluence $(if $(dirty),--dirty,)

play: $(OUTDIR)/$(STEM).mp4
	mpv --hwdec=no --vf=scale=426:616 $(OUTDIR)/$(STEM).mp4

confluence:
	cargo run --manifest-path drawio-lc/Cargo.toml -- $(YAML)

clean:
	rm -rf $(OUTDIR)

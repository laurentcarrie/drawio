.PHONY: all clean

all:
	cargo run --manifest-path drawio-lc/Cargo.toml -- drawio-lc/test.yaml

clean:
	rm -f *.png *.gif *.mp4 step*.drawio

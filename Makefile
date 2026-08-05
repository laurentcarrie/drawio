.PHONY: all clean

-include .env
export

all:
	cargo run --manifest-path drawio-lc/Cargo.toml -- test.yaml

clean:
	rm -f *.png *.gif *.mp4 *.html step*.drawio

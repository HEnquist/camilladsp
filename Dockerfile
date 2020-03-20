FROM rustembedded/cross:armv7-unknown-linux-gnueabihf

RUN dpkg --add-architecture armhf && \
    apt-get update && \
    apt-get install libasound2-dev:armhf -y && \
    apt-get install libpulse0 libpulse-dev:armhf -y \
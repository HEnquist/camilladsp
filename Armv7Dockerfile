FROM rustembedded/cross:armv7-unknown-linux-gnueabihf

RUN dpkg --add-architecture armv7 && \
    apt-get update && \
    apt-get install libasound2-dev:armv7 -y \
    apt-get install libpulse0 libpulse-dev:armv7 -y \
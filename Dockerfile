FROM rustembedded/cross:armv7-unknown-linux-gnueabihf

ENV PKG_CONFIG_ALLOW_CROSS 1
ENV PKG_CONFIG_PATH /usr/lib/arm-linux-gnueabihf/pkgconfig/

RUN dpkg --add-architecture armhf && \
    apt-get update && \
    apt-get install libasound2-dev:armhf -y && \
    apt-get install libpulse0 libpulse-dev:armhf -y \
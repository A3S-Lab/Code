FROM python:3.12-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1
ENV A3S_CODE_HARBOR_HOME=/opt/a3s-code-skillsbench
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /opt/src/a3s-code

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    build-essential \
    curl \
    git \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable
RUN rustc --version && cargo --version

COPY . /opt/src/a3s-code

RUN sed -i.bak \
    -e 's|a3s-common = { version = "0.1", path = "../../common" }|a3s-common = "0.1.1"|' \
    -e 's|a3s-memory = { version = "0.1.1", path = "../../memory" }|a3s-memory = "0.1.1"|' \
    -e 's|a3s-lane = { version = "0.4", path = "../../lane" }|a3s-lane = "0.4"|' \
    -e 's|a3s-search = { version = "0.9", path = "../../search", default-features = false }|a3s-search = { version = "0.9", default-features = false }|' \
    -e 's|a3s-box-sdk = { version = "0.7", path = "../../box/src/sdk", optional = true }|a3s-box-sdk = { version = "0.7", optional = true }|' \
    core/Cargo.toml \
    && rm -f core/Cargo.toml.bak \
    && sed -i.bak \
    -e '/a3s-ahp = { version = "0.1", path = "\.\.\/\.\.\/\.\.\/ahp" }/d' \
    sdk/python/Cargo.toml \
    && rm -f sdk/python/Cargo.toml.bak
RUN python -m pip install --upgrade pip setuptools wheel maturin
RUN python -m pip install harbor
RUN python -m pip install /opt/src/a3s-code/sdk/python

ENV PYTHONPATH="${A3S_CODE_HARBOR_HOME}:/usr/local/lib/python3.12/site-packages"
RUN mkdir -p "${A3S_CODE_HARBOR_HOME}" \
    && cp -R /opt/src/a3s-code/integrations/skillsbench "${A3S_CODE_HARBOR_HOME}/skillsbench"

WORKDIR /workspace

CMD ["python", "-c", "from skillsbench.harbor_a3s_agent import A3SCode; print(A3SCode.name())"]

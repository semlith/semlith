FROM debian:bookworm AS build

RUN echo build

FROM build AS runtime

COPY --from=build /app /app

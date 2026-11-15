# syntax=docker/dockerfile:1
ARG JAVA_VERSION=21
FROM eclipse-temurin:${JAVA_VERSION}-jdk AS build
WORKDIR /src
COPY --chown=app:app . .
RUN ./gradlew --no-daemon build \
    && echo "built"

FROM eclipse-temurin:21-jre
ENV APP_HOME=/opt/app LANG="C.UTF-8"
LABEL org.example.version="1.0"
EXPOSE 8080
COPY --from=build /src/build/libs/app.jar $APP_HOME/app.jar
USER 1000
ENTRYPOINT ["java", "-jar", "/opt/app/app.jar"]

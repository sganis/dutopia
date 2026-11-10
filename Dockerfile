FROM registry.access.redhat.com/ubi9/ubi

WORKDIR /app
COPY cloud .
USER 1001
EXPOSE 8080
CMD ["/app/duapi"]

#CMD ["sleep","infinity"]

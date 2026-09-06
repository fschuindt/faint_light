FROM python:3.12-slim

RUN pip install --no-cache-dir twirl astropy astroquery numpy scipy

COPY twirl_server.py /twirl_server.py

EXPOSE 8000
CMD ["python", "-u", "/twirl_server.py"]

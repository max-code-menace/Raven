-- Crée (ou remet à jour) l'utilisateur et la base d'Ubifacture. Peut être relancé sans risque.
-- À lancer avec le compte administrateur postgres (voir la commande dans la conversation).
-- CREATEDB permet aux tests de créer leurs propres bases temporaires.
SELECT format('CREATE ROLE ubifacture LOGIN CREATEDB PASSWORD %L', :'mdp')
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'ubifacture') \gexec

ALTER ROLE ubifacture WITH LOGIN CREATEDB PASSWORD :'mdp';

SELECT 'CREATE DATABASE ubifacture OWNER ubifacture ENCODING ''UTF8'' TEMPLATE template0'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'ubifacture') \gexec
